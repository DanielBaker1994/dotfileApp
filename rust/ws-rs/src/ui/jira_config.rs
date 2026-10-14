//! `JiraDashboardWindow` (kitchen_sink.swift), restored as the Rust **Jira
//! Config** window: the poll-jobs dashboard built on the shared card
//! scaffolding ([`crate::ui::card::CardWindowController::themed_root`]).
//!
//! The model is already ported in [`crate::views::jira`] (`Describe` /
//! `DescribeJob` / the dashboard helpers); this module is only the AppKit
//! window. Data comes from `jira_poll.py --describe` (no network, no writes).
//!
//! It renders a status line (polling on/off, last run, next poll), every poll
//! job with its interval and last-run time, the config / status / cache file
//! paths, and the poll controls (Enable / Disable, Force Poll, Stop Poll,
//! Setup…, Refresh). All user-facing strings and sizes come from
//! `commands.toml [jira-config]` (rule 3) — see [`JiraConfigSettings`].

use std::collections::HashMap;

use crate::views::jira::{Describe, DescribeJob};

// ===========================================================================
// Pure config + view model (AppKit-free, unit-tested)
// ===========================================================================

/// `[jira-config]` — the window's user-facing strings and sizes, read back
/// from `commands.toml` (rule 3).
#[derive(Clone, Debug, PartialEq)]
pub struct JiraConfigSettings {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub min_width: f64,
    pub min_height: f64,
    /// Auto-refresh period in seconds while the window is visible.
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
            title: "Jira Config".to_string(),
            width: 1080.0,
            height: 720.0,
            min_width: 860.0,
            min_height: 520.0,
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
            title: s("title", &d.title),
            width: num("dashboard-width", d.width),
            height: num("dashboard-height", d.height),
            min_width: num("dashboard-min-width", d.min_width),
            min_height: num("dashboard-min-height", d.min_height),
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

/// One poll job as the window shows it (name + interval + last run + next).
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub name: String,
    pub interval: String,
    pub last_run: String,
    pub next_run: String,
    pub status: String,
    pub enabled: bool,
}

/// The `Describe.endpoints` as display rows (the required core of the window).
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

/// The config / status / cache file paths the window lists (existing files
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
// AppKit window
// ===========================================================================

#[cfg(target_os = "macos")]
pub use appkit::show;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::app::host::SwitcherController;
    use crate::ui::card::CardWindowController;
    use crate::ui::popup::CardFrame;
    use crate::ui::theme::{PopupColors, PopupTone, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBezelStyle, NSButton, NSControlSize, NSEvent, NSEventMask,
        NSEventModifierFlags, NSFont, NSLineBreakMode, NSPopUpMenuWindowLevel, NSScrollView,
        NSText, NSTextField, NSView,
    };
    use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer};
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;

    const PAD: f64 = 16.0;
    const ROW_H: f64 = 22.0;
    const TITLE_H: f64 = 20.0;
    const LINE_H: f64 = 16.0;

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

    pub struct Ivars {
        window: Retained<crate::ui::card::CardNSWindow>,
        controller: Arc<SwitcherController>,
        doc: Retained<FlippedView>,
        scroll: Retained<NSScrollView>,
        describe: RefCell<Option<Describe>>,
        load_error: RefCell<String>,
        busy: Cell<bool>,
        timer: RefCell<Option<Retained<NSTimer>>>,
        monitor: RefCell<Option<Retained<AnyObject>>>,
        settings: JiraConfigSettings,
        colors: PopupColors,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraConfig"]
        #[ivars = Ivars]
        pub struct JiraConfig;

        impl JiraConfig {
            #[unsafe(method(enableClicked:))]
            fn enable_clicked(&self, _s: Option<&AnyObject>) {
                self.toggle_enable();
            }

            #[unsafe(method(forceClicked:))]
            fn force_clicked(&self, _s: Option<&AnyObject>) {
                self.run_script(vec!["--force".into(), "--quiet".into()]);
            }

            #[unsafe(method(stopClicked:))]
            fn stop_clicked(&self, _s: Option<&AnyObject>) {
                self.run_script(vec!["--cancel".into()]);
            }

            #[unsafe(method(setupClicked:))]
            fn setup_clicked(&self, _s: Option<&AnyObject>) {
                self.open_setup();
            }

            #[unsafe(method(refreshClicked:))]
            fn refresh_clicked(&self, _s: Option<&AnyObject>) {
                self.refresh();
            }
        }

        unsafe impl NSObjectProtocol for JiraConfig {}
    );

    thread_local! {
        /// The one live window (Swift's `JiraDashboardWindow.live`).
        static LIVE: RefCell<Option<Retained<JiraConfig>>> = const { RefCell::new(None) };
    }

    /// `JiraDashboardWindow.show(controller:)` — build once, re-raise after.
    pub fn show(controller: Arc<SwitcherController>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if let Some(w) = LIVE.with(|l| l.borrow().clone()) {
            w.refresh();
            NSApplication::sharedApplication(mtm).activate();
            w.ivars().window.makeKeyAndOrderFront(None);
            return;
        }
        let w = JiraConfig::build(mtm, controller);
        LIVE.with(|l| *l.borrow_mut() = Some(w.clone()));
        w.refresh();
        NSApplication::sharedApplication(mtm).activate();
        let win = &w.ivars().window;
        win.center();
        win.makeKeyAndOrderFront(None);
    }

    /// Close the live window (chrome ✕ / Esc / Cmd+W), clearing the state flag.
    fn close_live() {
        let Some(w) = LIVE.with(|l| l.borrow_mut().take()) else {
            return;
        };
        w.stop_timer();
        w.drop_monitor();
        w.ivars().controller.set_jira_config_shown(false);
        w.ivars().window.orderOut(None);
    }

    fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, color: Rgba) -> Retained<NSTextField> {
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

    impl JiraConfig {
        fn build(mtm: MainThreadMarker, controller: Arc<SwitcherController>) -> Retained<Self> {
            let settings = JiraConfigSettings::load();
            let colors = crate::ui::theme::section_colors("jira");
            let header_color = crate::ui::theme::section_header_color("jira");

            let frame = CardFrame {
                x: 0.0,
                y: 0.0,
                width: settings.width,
                height: settings.height,
            };
            let mut card = CardWindowController::new(
                mtm,
                frame,
                &settings.title,
                (settings.min_width, settings.min_height),
            );
            // The card chrome's ✕ closes only this window.
            *card.on_slot_hide.borrow_mut() = Some(Box::new(close_live));

            let scroll = NSScrollView::new(mtm);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDrawsBackground(false);
            let doc = FlippedView::new(mtm);
            doc.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
            scroll.setDocumentView(Some(&doc));

            card.themed_root(
                &scroll,
                "jira-config",
                colors,
                header_color,
                None,
                &settings.title,
                0.92,
            );
            card.window.setLevel(NSPopUpMenuWindowLevel + 1);

            let this = Self::alloc(mtm).set_ivars(Ivars {
                window: card.window.clone(),
                controller,
                doc,
                scroll,
                describe: RefCell::new(None),
                load_error: RefCell::new(String::new()),
                busy: Cell::new(false),
                timer: RefCell::new(None),
                monitor: RefCell::new(None),
                settings,
                colors,
            });
            let this: Retained<Self> = unsafe { objc2::msg_send![super(this), init] };
            this.install_monitor();
            this.start_timer();
            this
        }

        /// `startTimer()` — a repeating refresh while the window is visible
        /// (the timer's interval is `[jira-config] dashboard-refresh`). The
        /// block reads the live window, so it holds no strong cycle.
        fn start_timer(&self) {
            let secs = self.ivars().settings.refresh.max(2.0);
            let block = block2::RcBlock::new(move |_t: std::ptr::NonNull<NSTimer>| {
                let Some(w) = LIVE.with(|l| l.borrow().clone()) else {
                    return;
                };
                if w.ivars().window.isVisible() {
                    w.refresh();
                }
            });
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(secs, true, &block)
            };
            *self.ivars().timer.borrow_mut() = Some(timer);
        }

        fn stop_timer(&self) {
            if let Some(t) = self.ivars().timer.borrow_mut().take() {
                t.invalidate();
            }
        }

        /// Esc closes; Cmd+W closes; edit shortcuts reach the field editor
        /// (rule 1) — the jira_setup installEditShortcuts pattern.
        fn install_monitor(&self) {
            let block = block2::RcBlock::new(move |e: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
                let ev = unsafe { e.as_ref() };
                let Some((win, is_key)) = LIVE.with(|l| {
                    l.borrow().as_ref().map(|w| {
                        let win = w.ivars().window.clone();
                        let is_key = win.isKeyWindow();
                        (win, is_key)
                    })
                }) else {
                    return e.as_ptr();
                };
                if !is_key {
                    return e.as_ptr();
                }
                let m = ev.modifierFlags();
                let cmd = m.contains(NSEventModifierFlags::Command);
                let ctrl = m.contains(NSEventModifierFlags::Control);
                if ev.keyCode() == 53 {
                    close_live();
                    return std::ptr::null_mut();
                }
                if cmd && ev.keyCode() == 13 {
                    // Cmd+W
                    close_live();
                    return std::ptr::null_mut();
                }
                if !(cmd || ctrl) {
                    return e.as_ptr();
                }
                let Some(fr) = win.firstResponder() else {
                    return e.as_ptr();
                };
                let Some(ed) = (&*fr as &AnyObject).downcast_ref::<NSText>() else {
                    return e.as_ptr();
                };
                match (ev.keyCode(), cmd) {
                    (9, _) => unsafe { ed.paste(None) },
                    (8, _) => unsafe { ed.copy(None) },
                    (0, true) => unsafe { ed.selectAll(None) },
                    (7, true) => unsafe { ed.cut(None) },
                    (6, true) => {
                        if let Some(u) = ed.undoManager() {
                            u.undo();
                        }
                    }
                    _ => return e.as_ptr(),
                }
                std::ptr::null_mut()
            });
            let m = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block) };
            *self.ivars().monitor.borrow_mut() = m;
        }

        fn drop_monitor(&self) {
            if let Some(m) = self.ivars().monitor.borrow_mut().take() {
                unsafe { NSEvent::removeMonitor(&m) };
            }
        }

        // -- actions -------------------------------------------------------

        fn toggle_enable(&self) {
            let name = if crate::app::jira_poll::enabled_in_config() {
                "jira-poll-off"
            } else {
                "jira-poll-on"
            };
            self.ivars().controller.jira_poll(name);
        }

        fn open_setup(&self) {
            let ctrl = self.ivars().controller.clone();
            crate::ui::jira_setup::show(None, Box::new(move || ctrl.jira_enable_result(true, "")));
        }

        /// Run `jira_poll.py ARGS` off the main thread, then refresh.
        fn run_script(&self, args: Vec<String>) {
            if self.ivars().busy.get() {
                return;
            }
            self.ivars().busy.set(true);
            let refs: Vec<String> = args;
            std::thread::spawn(move || {
                let a: Vec<&str> = refs.iter().map(String::as_str).collect();
                let (code, _out, err) = crate::app::jira_poll::run("jira_poll.py", &a);
                let msg = if code == 0 {
                    String::new()
                } else {
                    crate::app::jira_poll::error_line(&err, &format!("exit {code}"))
                };
                crate::platform::run_on_main(move || {
                    if let Some(w) = LIVE.with(|l| l.borrow().clone()) {
                        w.ivars().busy.set(false);
                        if !msg.is_empty() {
                            *w.ivars().load_error.borrow_mut() = format!("✗ {msg}");
                        }
                        w.refresh();
                    }
                });
            });
        }

        /// `refresh()` — `jira_poll.py --describe` off-thread, then render.
        fn refresh(&self) {
            if self.ivars().busy.get() {
                return;
            }
            self.ivars().busy.set(true);
            std::thread::spawn(|| {
                let res = crate::views::jira::run_jira_poll(&["--describe"], None);
                crate::platform::run_on_main(move || {
                    let Some(w) = LIVE.with(|l| l.borrow().clone()) else {
                        return;
                    };
                    w.ivars().busy.set(false);
                    match res {
                        Ok(v) => {
                            *w.ivars().load_error.borrow_mut() = String::new();
                            *w.ivars().describe.borrow_mut() = Some(Describe::from_value(&v));
                        }
                        Err(e) => {
                            *w.ivars().load_error.borrow_mut() = format!("✗ {e}");
                        }
                    }
                    w.render();
                });
            });
        }

        // -- rendering -----------------------------------------------------

        fn color(&self, tone: &str) -> Rgba {
            let c = self.ivars().colors;
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

        fn title_at(&self, mtm: MainThreadMarker, text: &str, y: f64, w: f64) -> f64 {
            let l = label(mtm, &text.to_uppercase(), 10.0, true, self.ivars().colors.dim);
            l.setFrame(rect(PAD, y, w, LINE_H));
            self.add(&l);
            y + TITLE_H
        }

        /// Rebuild the document view from the current state.
        fn render(&self) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            self.clear();
            let colors = self.ivars().colors;
            let s = &self.ivars().settings;
            let doc_w = (self.ivars().scroll.bounds().size.width).max(s.width - 4.0);
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
            let sl = label(mtm, &status, 13.0, true, self.color(tone));
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

            // Poll jobs (§ every poll job, interval + last run).
            y = self.title_at(mtm, "Poll jobs", y + 6.0, doc_w);
            let rows = describe.as_ref().map(job_rows).unwrap_or_default();
            if rows.is_empty() {
                let text = if describe.is_none() {
                    "Loading poll jobs…"
                } else {
                    "No poll jobs configured — open Setup…"
                };
                let l = label(mtm, text, 12.0, false, colors.dim);
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
                    let dot = label(mtm, "●", 9.0, false, dot_color);
                    dot.setFrame(rect(PAD, y + 3.0, 12.0, LINE_H));
                    self.add(&dot);
                    let name = label(mtm, &r.name, 12.5, true, colors.text);
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
                    let d = label(mtm, &detail, 11.5, false, colors.dim);
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
                let l = label(mtm, "Loading paths…", 11.5, false, colors.dim);
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
            let specs: [(&str, objc2::runtime::Sel); 5] = [
                (enable.as_str(), sel!(enableClicked:)),
                (s.force_label.as_str(), sel!(forceClicked:)),
                (s.stop_label.as_str(), sel!(stopClicked:)),
                (s.setup_label.as_str(), sel!(setupClicked:)),
                (s.refresh_label.as_str(), sel!(refreshClicked:)),
            ];
            let target: &AnyObject = as_any(self);
            let mut x = PAD;
            for (title, action) in specs {
                let b = NSButton::new(mtm);
                b.setTitle(&NSString::from_str(title));
                b.setBezelStyle(NSBezelStyle::Push);
                b.setControlSize(NSControlSize::Small);
                b.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                let w = (title.chars().count() as f64 * 7.2 + 26.0).max(84.0);
                b.setFrame(rect(x, y, w, 28.0));
                if action == sel!(stopClicked:) {
                    b.setToolTip(Some(&NSString::from_str("Stop the running poll (jira_poll.py --cancel)")));
                    b.setEnabled(lock_held);
                }
                if action == sel!(forceClicked:) {
                    b.setToolTip(Some(&NSString::from_str("Poll every enabled job now (jira_poll.py --force)")));
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

    impl std::fmt::Debug for JiraConfig {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("JiraConfig").finish()
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
        m.insert("title".to_string(), "Jira Board".to_string());
        m.insert("dashboard-width".to_string(), "1200".to_string());
        m.insert("dashboard-refresh".to_string(), "9".to_string());
        m.insert("force-poll-label".to_string(), "Poll Now".to_string());
        let s = JiraConfigSettings::from_section(&m);
        assert_eq!(s.title, "Jira Board");
        assert_eq!(s.width, 1200.0);
        assert_eq!(s.refresh, 9.0);
        assert_eq!(s.force_label, "Poll Now");
        // Untouched keys keep their defaults.
        assert_eq!(s.height, d.height);
        assert_eq!(s.stop_label, d.stop_label);
    }

    #[test]
    fn from_text_reads_only_the_jira_config_section() {
        let text = "\
[app]
dashboard-width = 1

[jira-config]
enabled = true
in-palette = true
dashboard-width = 1100
dashboard-height = 640
enable-label = \"Turn On\"

[jira]
enabled = true
";
        let s = JiraConfigSettings::from_text(text);
        assert_eq!(s.width, 1100.0);
        assert_eq!(s.height, 640.0);
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
