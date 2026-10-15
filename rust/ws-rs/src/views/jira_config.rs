//! The Jira **Config** surface: the poll-jobs dashboard the Swift
//! `JiraDashboardWindow` showed, now the `Config` half of the Jira view's
//! `Tickets | Config` pill (`views/jira_view.rs`) — no separate window.
//!
//! The pure, config-driven model lives here ([`JiraConfigSettings`],
//! [`job_rows`], [`next_poll`], [`status_line`], [`file_paths`]); the AppKit
//! half ([`appkit::JiraConfigBodyView`]) paints the status line, every poll
//! job with its interval + last run, the config / status / log paths and the
//! Enable / Disable, Force Poll, Stop Poll, Setup…, Refresh controls, in place
//! of the ticket list.
//!
//! Data comes from `jira_poll.py --describe` (no network, no writes) but the
//! fetch is owned by the host (`app/host.rs`), which hands the parse back
//! through `JiraView::set_config_result`; the body only renders and reports
//! control presses as [`JiraConfigAction`]s. All user-facing strings and the
//! auto-refresh interval come from `commands.toml [jira-config]` (rule 3).

use std::collections::HashMap;

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
    /// Open the Jira setup window.
    Setup,
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
    pub setup_label: String,
    pub refresh_label: String,
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
            setup_label: "Setup…".to_string(),
            refresh_label: "Refresh".to_string(),
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
            setup_label: s("setup-button-label", &d.setup_label),
            refresh_label: s("refresh-button-label", &d.refresh_label),
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

/// The status line + a tone key (`text` / `dim` / `warn` / `err`):
/// polling on/off, last run, and the next poll.
pub fn status_line(
    d: &Describe,
    enabled: bool,
    poll_active: bool,
    lock_held: bool,
) -> (String, &'static str) {
    if !enabled {
        return if poll_active {
            ("◐ Polling in the background (Jira window off)".to_string(), "dim")
        } else {
            ("○ Polling off".to_string(), "dim")
        };
    }
    let last = if d.last_run.trim().is_empty() {
        "never".to_string()
    } else {
        d.last_run.trim().to_string()
    };
    let head = if lock_held {
        "● Polling now…"
    } else {
        "● Polling on"
    };
    (
        format!("{head} — last run {last} · next {}", next_poll(d)),
        "text",
    )
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
// AppKit body (drawn into the Jira view's list area)
// ===========================================================================

#[cfg(target_os = "macos")]
pub use appkit::JiraConfigBodyView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::theme::{PopupColors, PopupTone, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSControlSize, NSFont, NSLineBreakMode,
        NSScrollView, NSTextField, NSView,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const PAD: f64 = 16.0;
    const ROW_H: f64 = 22.0;
    const TITLE_H: f64 = 20.0;
    const LINE_H: f64 = 16.0;

    /// The event sink: one [`JiraConfigAction`] per control press.
    pub type ActionSink = Rc<dyn Fn(JiraConfigAction)>;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)))
    }

    fn as_any<T: objc2::Message + ?Sized>(o: &T) -> &AnyObject {
        unsafe { &*(o as *const T as *const AnyObject) }
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

            #[unsafe(method(setupClicked:))]
            fn setup_clicked(&self, _s: Option<&AnyObject>) {
                self.emit(JiraConfigAction::Setup);
            }

            #[unsafe(method(refreshClicked:))]
            fn refresh_clicked(&self, _s: Option<&AnyObject>) {
                self.emit(JiraConfigAction::Refresh);
            }
        }

        unsafe impl NSObjectProtocol for JiraConfigBodyView {}
    );

    impl JiraConfigBodyView {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let scroll = NSScrollView::new(mtm);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDrawsBackground(false);
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let doc = FlippedView::new(mtm);
            doc.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            scroll.setDocumentView(Some(&doc));

            let this = Self::alloc(mtm).set_ivars(BodyIvars {
                scroll: scroll.clone(),
                doc,
                describe: RefCell::new(None),
                load_error: RefCell::new(String::new()),
                settings: JiraConfigSettings::load(),
                colors: Cell::new(colors),
                timer: RefCell::new(None),
                sink: RefCell::new(None),
                active: Cell::new(false),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            v.addSubview(&scroll);
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
            *self.ivars().sink.borrow_mut() = Some(Rc::from(cb));
        }

        /// `true` while the Config tab is selected: the auto-refresh only runs
        /// then.
        pub fn set_active(&self, on: bool) {
            self.ivars().active.set(on);
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
            let doc = &self.ivars().doc;
            for v in doc.subviews().iter() {
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

        fn title_at(&self, mtm: MainThreadMarker, text: &str, y: f64, w: f64) -> f64 {
            let l = self.label(mtm, &text.to_uppercase(), 10.0, true, self.ivars().colors.get().dim);
            l.setFrame(rect(PAD, y, w, LINE_H));
            self.add(&l);
            y + TITLE_H
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
                    Some(d) => status_line(d, enabled, poll_active, d.lock_held),
                    None => ("Loading…".to_string(), "dim"),
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

            // Poll jobs (every poll job, interval + last run).
            y = self.title_at(mtm, "Poll jobs", y + 6.0, doc_w);
            let rows = describe.as_ref().map(job_rows).unwrap_or_default();
            if rows.is_empty() {
                let text = if describe.is_none() {
                    "Loading poll jobs…"
                } else {
                    "No poll jobs configured — open Setup…"
                };
                let l = self.label(mtm, text, 12.0, false, colors.dim);
                l.setFrame(rect(PAD + 4.0, y, doc_w - PAD * 2.0, LINE_H));
                self.add(&l);
                y += ROW_H;
            } else {
                for r in &rows {
                    let dot_color = if !r.enabled {
                        colors.tone(PopupTone::Dim)
                    } else {
                        match r.status.as_str() {
                            "ok" | "success" => colors.tone(PopupTone::Success),
                            "error" | "failed" => colors.tone(PopupTone::Danger),
                            "running" => colors.tone(PopupTone::Warning),
                            "never run" | "never" => colors.tone(PopupTone::Dim),
                            _ => colors.tone(PopupTone::Success),
                        }
                    };
                    let dot = self.label(mtm, "●", 9.0, false, dot_color);
                    dot.setFrame(rect(PAD, y + 3.0, 12.0, LINE_H));
                    self.add(&dot);
                    let name = self.label(mtm, &r.name, 12.5, true, colors.text);
                    name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                    name.setFrame(rect(PAD + 16.0, y, 220.0, LINE_H));
                    self.add(&name);
                    let detail = format!(
                        "every {} · last {} · next {} · {}{}",
                        r.interval,
                        r.last_run,
                        r.next_run,
                        r.status,
                        if r.enabled { "" } else { " (off)" },
                    );
                    let d = self.label(mtm, &detail, 11.5, false, colors.dim);
                    d.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                    d.setFrame(rect(PAD + 240.0, y + 1.0, (doc_w - PAD * 2.0 - 224.0).max(1.0), LINE_H));
                    self.add(&d);
                    y += ROW_H;
                }
            }

            // Files.
            y = self.title_at(mtm, "Files", y + 10.0, doc_w);
            let paths = describe.as_ref().map(file_paths).unwrap_or_default();
            if paths.is_empty() {
                let l = self.label(mtm, "Loading paths…", 11.5, false, colors.dim);
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

            let h = (y + PAD).max(self.ivars().scroll.bounds().size.height);
            self.ivars().doc.setFrame(rect(0.0, 0.0, doc_w, h));
        }

        fn render_controls(&self, mtm: MainThreadMarker, y: f64) -> f64 {
            let s = &self.ivars().settings;
            let enabled = crate::app::jira_poll::enabled_in_config();
            let lock_held = self
                .ivars()
                .describe
                .borrow()
                .as_ref()
                .map(|d| d.lock_held)
                .unwrap_or(false);
            let enable = if enabled { &s.disable_label } else { &s.enable_label };
            let specs: [(&str, objc2::runtime::Sel, JiraConfigAction); 5] = [
                (enable.as_str(), sel!(enableClicked:), JiraConfigAction::Enable),
                (s.force_label.as_str(), sel!(forceClicked:), JiraConfigAction::Force),
                (s.stop_label.as_str(), sel!(stopClicked:), JiraConfigAction::Stop),
                (s.setup_label.as_str(), sel!(setupClicked:), JiraConfigAction::Setup),
                (s.refresh_label.as_str(), sel!(refreshClicked:), JiraConfigAction::Refresh),
            ];
            let target: &AnyObject = as_any(self);
            let mut x = PAD;
            for (title, action, kind) in specs {
                let b = NSButton::new(mtm);
                b.setTitle(&NSString::from_str(title));
                b.setBezelStyle(NSBezelStyle::Push);
                b.setControlSize(NSControlSize::Small);
                b.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                let w = (title.chars().count() as f64 * 7.2 + 26.0).max(84.0);
                b.setFrame(rect(x, y, w, 28.0));
                match kind {
                    JiraConfigAction::Stop => {
                        b.setToolTip(Some(&NSString::from_str("Stop the running poll (jira_poll.py --cancel)")));
                        b.setEnabled(lock_held);
                    }
                    JiraConfigAction::Force => {
                        b.setToolTip(Some(&NSString::from_str("Poll every enabled job now (jira_poll.py --force)")));
                    }
                    _ => {}
                }
                unsafe {
                    b.setTarget(Some(target));
                    b.setAction(Some(action));
                }
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
    fn status_line_reports_on_off_and_last_run() {
        let d = describe(json!({
            "lastRun": "2026-10-14 12:00:00",
            "endpoints": [{"name": "a", "enabled": true, "nextRun": "2026-10-14 12:05:00"}]
        }));
        let (line, tone) = status_line(&d, true, false, false);
        assert!(line.starts_with("● Polling on"));
        assert!(line.contains("last run 2026-10-14 12:00:00"));
        assert!(line.contains("next 2026-10-14 12:05:00"));
        assert_eq!(tone, "text");

        let (off, _) = status_line(&d, false, false, false);
        assert_eq!(off, "○ Polling off");
        let (bg, _) = status_line(&d, false, true, false);
        assert!(bg.starts_with("◐ Polling in the background"));

        let (now, _) = status_line(&d, true, false, true);
        assert!(now.starts_with("● Polling now…"));
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
}
