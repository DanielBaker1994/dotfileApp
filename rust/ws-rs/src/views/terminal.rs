//! `/terminal` panel — mirrors `TerminalPanel.swift` (the SwiftTerm-backed
//! borderless non-activating floating terminal).
//!
//! The pure model (config + session state + placement) is complete and tested;
//! [`TerminalPanel::build`] constructs the real `PopupPanel` + the SwiftTerm
//! shim view on macOS (a no-op elsewhere).

use objc2::MainThreadMarker;
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;

pub const DEFAULT_FRAME_WIDTH: f64 = 760.0;
pub const DEFAULT_FRAME_HEIGHT: f64 = 460.0;
pub const HEADER_HEIGHT: f64 = 30.0;
pub const MIN_WIDTH: f64 = 360.0;
pub const MIN_HEIGHT: f64 = 200.0;
pub const MIN_FONT_SIZE: f64 = 8.0;
pub const MAX_FONT_SIZE: f64 = 36.0;
pub const DEFAULT_FONT_SIZE: f64 = 12.0;
pub const CORNER_RADIUS: f64 = 12.0;
pub const NO_EXIT_CODE: i32 = -1;

/// `TerminalPanel.frameKey` — the remembered panel frame (user defaults).
pub const FRAME_KEY: &str = "terminalPanelFrame";

/// `NSStringFromRect` — `{{x, y}, {w, h}}`.
pub fn format_ns_rect(f: TerminalFrame) -> String {
    format!("{{{{{}, {}}}, {{{}, {}}}}}", f.x, f.y, f.width, f.height)
}

/// `NSRectFromString` for the `{{x, y}, {w, h}}` form (`None` otherwise).
pub fn parse_ns_rect(s: &str) -> Option<TerminalFrame> {
    let nums: Vec<f64> = s
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    match nums.as_slice() {
        [x, y, w, h] => Some(TerminalFrame::new(*x, *y, *w, *h)),
        _ => None,
    }
}

/// A floating-panel frame (`NSRect` in points) kept AppKit-free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl TerminalFrame {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        TerminalFrame { x, y, width, height }
    }

    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }

    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
}

impl Default for TerminalFrame {
    fn default() -> Self {
        TerminalFrame::new(0.0, 0.0, DEFAULT_FRAME_WIDTH, DEFAULT_FRAME_HEIGHT)
    }
}

/// `[app] shell` + `terminal-font` / `terminal-font-size` plus the remembered
/// panel frame (`terminalPanelFrame`).
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalConfig {
    pub shell: String,
    pub args: Vec<String>,
    pub font_name: String,
    pub font_size: f64,
    pub frame: TerminalFrame,
    pub start_open: bool,
}

impl TerminalConfig {
    pub fn new(
        shell: impl Into<String>,
        args: Vec<String>,
        font_name: impl Into<String>,
        font_size: f64,
    ) -> Self {
        TerminalConfig {
            shell: shell.into(),
            args,
            font_name: font_name.into(),
            font_size: clamp_font_size(font_size),
            frame: TerminalFrame::default(),
            start_open: true,
        }
    }

    /// Build from `[app]` settings (`shell` / `shell-args` / `terminal-font` /
    /// `terminal-font-size`), the values `TerminalPanel.swift` is seeded with.
    pub fn from_settings(settings: &crate::app::config::AppSettings) -> Self {
        TerminalConfig {
            shell: settings.shell.clone(),
            args: settings.shell_args.clone(),
            font_name: settings.terminal_font.clone(),
            font_size: clamp_font_size(settings.terminal_font_size),
            frame: TerminalFrame::default(),
            start_open: true,
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()),
            args: vec!["-l".to_string()],
            font_name: "Menlo".to_string(),
            font_size: DEFAULT_FONT_SIZE,
            frame: TerminalFrame::default(),
            start_open: true,
        }
    }
}

/// Clamp to the Swift panel's `min(36, max(8, size))`.
pub fn clamp_font_size(size: f64) -> f64 {
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// Parse a `"Menlo 13"`-style descriptor; a missing size falls back to the
/// default, a non-positive size is rejected.
pub fn parse_font_descriptor(s: &str) -> (String, f64) {
    let s = s.trim();
    match s.rsplit_once(char::is_whitespace) {
        Some((name, size)) if !name.trim().is_empty() => {
            match size.trim().parse::<f64>() {
                Ok(n) if n > 0.0 => (name.trim().to_string(), clamp_font_size(n)),
                _ => (s.to_string(), DEFAULT_FONT_SIZE),
            }
        }
        _ => (s.to_string(), DEFAULT_FONT_SIZE),
    }
}

/// Session state the Swift panel reads from the SwiftTerm view + its delegate.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalSession {
    pub running: bool,
    pub exit_code: i32,
    pub cols: usize,
    pub rows: usize,
    pub title: String,
    pub cwd: Option<String>,
}

impl Default for TerminalSession {
    fn default() -> Self {
        TerminalSession {
            running: false,
            exit_code: NO_EXIT_CODE,
            cols: 0,
            rows: 0,
            title: String::new(),
            cwd: None,
        }
    }
}

/// `/terminal` panel model: owns the config, the session state and (once built)
/// the SwiftTerm shim `NSView` + its `NSPanel`.
#[derive(Default)]
pub struct TerminalPanel {
    config: TerminalConfig,
    session: TerminalSession,
    #[cfg(target_os = "macos")]
    view: Option<Retained<NSView>>,
    #[cfg(target_os = "macos")]
    panel: Option<Retained<crate::ui::popup::PopupPanel>>,
    shown: bool,
    key: bool,
    window_number: i64,
    level: i64,
}

impl TerminalPanel {
    pub fn new(config: TerminalConfig) -> Self {
        TerminalPanel {
            config,
            session: TerminalSession::default(),
            #[cfg(target_os = "macos")]
            view: None,
            #[cfg(target_os = "macos")]
            panel: None,
            shown: false,
            key: false,
            window_number: 0,
            level: 0,
        }
    }

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    pub fn session(&self) -> &TerminalSession {
        &self.session
    }

    /// The shim view once [`Self::build`] has run.
    #[cfg(target_os = "macos")]
    pub fn view(&self) -> Option<&NSView> {
        self.view.as_deref()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn view(&self) -> Option<&()> {
        None
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    pub fn is_key(&self) -> bool {
        self.key
    }

    pub fn frame(&self) -> TerminalFrame {
        self.config.frame
    }

    pub fn toggle(&mut self) {
        if self.shown && self.key {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn show(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(mtm) = MainThreadMarker::new() {
                self.build(mtm);
                // `show()`: `if !panel.isVisible { place() }`.
                if let Some(panel) = &self.panel {
                    if !panel.isVisible() {
                        if let Some(f) = Self::placed_frame(mtm) {
                            panel.setFrame_display(
                                objc2_foundation::NSRect::new(
                                    objc2_foundation::NSPoint::new(f.x, f.y),
                                    objc2_foundation::NSSize::new(f.width, f.height),
                                ),
                                false,
                            );
                            self.config.frame = f;
                        }
                    }
                }
            }
            if let Some(panel) = &self.panel {
                panel.makeKeyAndOrderFront(None);
                self.window_number = panel.windowNumber() as i64;
            }
        }
        self.shown = true;
    }

    pub fn hide(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            // `saveFrame()` (Swift saves on move/resize; the frame is the
            // same by the time the panel hides).
            if panel.isVisible() {
                let f = panel.frame();
                Self::save_frame(TerminalFrame::new(
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                ));
            }
            panel.orderOut(None);
        }
        self.shown = false;
        self.key = false;
    }

    /// Test/state seam for the key-window flag the real panel reads off `NSPanel`.
    pub fn set_key(&mut self, key: bool) {
        self.key = key;
    }

    /// Apply a SwiftTerm delegate hook (`WSShimDelegate.shimEvent`).
    pub fn handle_event(&mut self, kind: &str, value: &str) {
        match kind {
            "size" => {
                if let Some((c, r)) = value.split_once('x') {
                    if let (Ok(c), Ok(r)) = (c.parse(), r.parse()) {
                        self.session.cols = c;
                        self.session.rows = r;
                    }
                }
            }
            "title" => self.session.title = value.to_string(),
            "cwd" => {
                self.session.cwd = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "exit" => {
                self.session.running = false;
                self.session.exit_code = value.parse().unwrap_or(NO_EXIT_CODE);
            }
            _ => {}
        }
    }

    /// `place()`'s live inputs: the visible frame of the screen under the
    /// mouse and the `terminalPanelFrame` user default.
    #[cfg(target_os = "macos")]
    fn placed_frame(mtm: MainThreadMarker) -> Option<TerminalFrame> {
        use objc2_app_kit::{NSEvent, NSScreen};
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let screen = screens
            .iter()
            .find(|s| {
                let f = s.frame();
                mouse.x >= f.origin.x
                    && mouse.x < f.origin.x + f.size.width
                    && mouse.y >= f.origin.y
                    && mouse.y < f.origin.y + f.size.height
            })
            .or_else(|| NSScreen::mainScreen(mtm))?;
        let vf = screen.visibleFrame();
        let visible = TerminalFrame::new(vf.origin.x, vf.origin.y, vf.size.width, vf.size.height);
        Some(Self::place(Self::saved_frame(), visible))
    }

    #[cfg(target_os = "macos")]
    fn saved_frame() -> Option<TerminalFrame> {
        use objc2_foundation::{NSString, NSUserDefaults};
        let d = NSUserDefaults::standardUserDefaults();
        let s = d.stringForKey(&NSString::from_str(FRAME_KEY))?;
        parse_ns_rect(&s.to_string())
    }

    #[cfg(target_os = "macos")]
    fn save_frame(f: TerminalFrame) {
        use objc2_foundation::{NSString, NSUserDefaults};
        let d = NSUserDefaults::standardUserDefaults();
        let v = NSString::from_str(&format_ns_rect(f));
        unsafe { d.setObject_forKey(Some(&v), &NSString::from_str(FRAME_KEY)) };
    }

    /// `TerminalPanel.place()`: reuse the saved frame when it still fits the
    /// screen, else clamp its size and center it just below the middle.
    pub fn place(saved: Option<TerminalFrame>, visible: TerminalFrame) -> TerminalFrame {
        let mut frame = TerminalFrame::default();
        if let Some(saved) = saved {
            if saved.width >= MIN_WIDTH && saved.height >= MIN_HEIGHT {
                // Swift: `vf.intersects(saved) && vf.contains(saved.mid)`.
                let fits = saved.mid_x() >= visible.x
                    && saved.mid_x() < visible.x + visible.width
                    && saved.mid_y() >= visible.y
                    && saved.mid_y() < visible.y + visible.height;
                if fits {
                    return saved;
                }
                frame.width = saved.width.min(visible.width);
                frame.height = saved.height.min(visible.height);
            }
        }
        frame.x = visible.x + (visible.width - frame.width) / 2.0;
        frame.y = visible.y + (visible.height - frame.height) / 2.0 + visible.height * 0.08;
        frame
    }

    /// Build the AppKit panel and the SwiftTerm shim view, then start the shell.
    /// No-op off macOS.
    pub fn build(&mut self, mtm: MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.build_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
    }

    #[cfg(target_os = "macos")]
    fn build_macos(&mut self, mtm: MainThreadMarker) {
        use crate::app::config;
        use crate::ui::popup::{CardFrame, PopupConfig, PopupPanel};
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        if self.view.is_some() {
            return;
        }

        let mut settings = config::AppSettings::default();
        config::apply_app_config_from_disk(&mut settings);
        self.config = TerminalConfig::from_settings(&settings);

        let f = self.config.frame;
        let rect = NSRect::new(NSPoint::new(f.x, f.y), NSSize::new(f.width, f.height));
        let cfg = PopupConfig {
            name: "terminal".to_string(),
            frame: CardFrame { x: f.x, y: f.y, width: f.width, height: f.height },
            corner_radius: CORNER_RADIUS,
            tool_panel: true,
            floating: true,
            enable_resize: true,
            ..PopupConfig::default()
        };
        let panel = PopupPanel::create(mtm, &cfg);
        let view = swiftterm_shim::create_terminal(mtm, rect);
        panel.setContentView(Some(&view));

        if let Some(shim) = view.clone().downcast::<swiftterm_shim::WSShim>().ok() {
            swiftterm_shim::set_font(&shim, &self.config.font_name, self.config.font_size);
            let home = std::env::var("HOME").ok();
            swiftterm_shim::start_process(
                &shim,
                &self.config.shell,
                &self.config.args,
                home.as_deref(),
            );
        }
        self.session.running = true;
        self.panel = Some(panel);
        self.view = Some(view);
    }

    /// `TerminalAutoRestart`: `exit` in the panel's shell starts a new one in
    /// the same view (Swift restarts in `$HOME`). True when it restarted.
    #[cfg(target_os = "macos")]
    pub fn poll_restart(&mut self) -> bool {
        let Some(shim) = self.shim() else {
            return false;
        };
        let home = std::env::var("HOME").ok();
        swiftterm_shim::restart_if_exited(&shim, &self.config.shell, &self.config.args, home.as_deref())
    }

    /// The shim handle behind [`Self::view`] (it IS the returned `NSView`).
    #[cfg(target_os = "macos")]
    fn shim(&self) -> Option<Retained<swiftterm_shim::WSShim>> {
        self.view.as_ref()?.clone().downcast::<swiftterm_shim::WSShim>().ok()
    }

    /// Send raw keys/text to the shell (shim `sendKeys:`).
    pub fn send_keys(&mut self, text: &str) {
        #[cfg(target_os = "macos")]
        if let Some(shim) = self.shim() {
            swiftterm_shim::send_text(&shim, text);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = text;
    }

    /// `setFont(name:size:)` — updates the config now, applies it when built.
    pub fn set_font(&mut self, name: &str, size: f64) {
        self.config.font_name = name.to_string();
        self.config.font_size = clamp_font_size(size);
        #[cfg(target_os = "macos")]
        if let Some(shim) = self.shim() {
            swiftterm_shim::set_font(&shim, &self.config.font_name, self.config.font_size);
        }
    }

    /// The panel `testState()` JSON (socket `terminalPanel`), read live from
    /// the `NSPanel` + shim once built (Swift reads `panel.isVisible`,
    /// `isKeyWindow`, `level`, `windowNumber`, `frame`, `process.running`).
    pub fn test_state(&self) -> Value {
        let mut doc = self.model_state();
        #[cfg(target_os = "macos")]
        if let (Some(panel), Some(o)) = (&self.panel, doc.as_object_mut()) {
            let f = panel.frame();
            o.insert("shown".into(), json!(panel.isVisible()));
            o.insert("key".into(), json!(panel.isKeyWindow()));
            o.insert("level".into(), json!(panel.level() as i64));
            o.insert("wid".into(), json!(panel.windowNumber() as i64));
            o.insert(
                "frame".into(),
                json!([
                    f.origin.x as i64,
                    f.origin.y as i64,
                    f.size.width as i64,
                    f.size.height as i64
                ]),
            );
            if let Some(shim) = self.shim() {
                o.insert("running".into(), json!(shim.terminal_running()));
            }
        }
        doc
    }

    /// The mirrored-model half of [`Self::test_state`] (headless tests).
    fn model_state(&self) -> Value {
        json!({
            "shown": self.shown,
            "key": self.key,
            "level": self.level,
            "wid": self.window_number,
            "frame": [
                self.config.frame.x as i64,
                self.config.frame.y as i64,
                self.config.frame.width as i64,
                self.config.frame.height as i64,
            ],
            "running": self.session.running,
            "exitCode": self.session.exit_code,
            "cols": self.session.cols,
            "rows": self.session.rows,
            "title": self.session.title,
            "cwd": self.session.cwd,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_clamp() {
        let cfg = TerminalConfig::default();
        assert_eq!(cfg.frame, TerminalFrame::default());
        assert_eq!(cfg.font_size, DEFAULT_FONT_SIZE);
        assert_eq!(cfg.args, vec!["-l".to_string()]);

        let cfg = TerminalConfig::new("/bin/bash", vec![], "Menlo", 100.0);
        assert_eq!(cfg.font_size, MAX_FONT_SIZE);
        let cfg = TerminalConfig::new("/bin/bash", vec![], "Menlo", 1.0);
        assert_eq!(cfg.font_size, MIN_FONT_SIZE);
        assert_eq!(clamp_font_size(13.0), 13.0);
    }

    #[test]
    fn config_from_settings_maps_app_values() {
        let mut settings = crate::app::config::AppSettings::default();
        settings.shell = "/bin/fish".to_string();
        settings.shell_args = vec!["-l".to_string()];
        settings.terminal_font = "Fira Code".to_string();
        settings.terminal_font_size = 200.0;

        let cfg = TerminalConfig::from_settings(&settings);
        assert_eq!(cfg.shell, "/bin/fish");
        assert_eq!(cfg.args, vec!["-l".to_string()]);
        assert_eq!(cfg.font_name, "Fira Code");
        assert_eq!(cfg.font_size, MAX_FONT_SIZE);
    }

    #[test]
    fn font_descriptor_parsing() {
        assert_eq!(
            parse_font_descriptor("Menlo 13"),
            ("Menlo".to_string(), 13.0)
        );
        assert_eq!(
            parse_font_descriptor("JetBrains Mono 100"),
            ("JetBrains Mono".to_string(), MAX_FONT_SIZE)
        );
        assert_eq!(
            parse_font_descriptor("Menlo"),
            ("Menlo".to_string(), DEFAULT_FONT_SIZE)
        );
        assert_eq!(
            parse_font_descriptor("Menlo nope"),
            ("Menlo nope".to_string(), DEFAULT_FONT_SIZE)
        );
    }

    #[test]
    fn toggle_tracks_shown_and_key() {
        let mut p = TerminalPanel::new(TerminalConfig::default());
        assert!(!p.is_shown());
        p.toggle();
        assert!(p.is_shown());
        // Shown but not key (the panel model doesn't own the key window).
        p.toggle();
        assert!(p.is_shown());
        p.set_key(true);
        p.toggle();
        assert!(!p.is_shown());
        assert!(!p.is_key());
    }

    #[test]
    fn delegate_events_update_session() {
        let mut p = TerminalPanel::new(TerminalConfig::default());
        p.handle_event("size", "120x40");
        p.handle_event("title", "zsh");
        p.handle_event("cwd", "/tmp");
        p.handle_event("exit", "0");
        let s = p.session();
        assert_eq!((s.cols, s.rows), (120, 40));
        assert_eq!(s.title, "zsh");
        assert_eq!(s.cwd.as_deref(), Some("/tmp"));
        assert_eq!(s.exit_code, 0);
        assert!(!s.running);

        p.handle_event("cwd", "");
        assert_eq!(p.session().cwd, None);
        p.handle_event("size", "garbage");
        assert_eq!(p.session().rows, 40);
    }

    #[test]
    fn set_font_updates_config_only() {
        let mut p = TerminalPanel::new(TerminalConfig::default());
        p.set_font("Fira Code", 200.0);
        assert_eq!(p.config().font_name, "Fira Code");
        assert_eq!(p.config().font_size, MAX_FONT_SIZE);
        assert!(p.view().is_none());
    }

    #[test]
    fn ns_rect_strings_round_trip() {
        let f = TerminalFrame::new(120.0, -40.5, 760.0, 460.0);
        assert_eq!(format_ns_rect(f), "{{120, -40.5}, {760, 460}}");
        assert_eq!(parse_ns_rect(&format_ns_rect(f)), Some(f));
        assert_eq!(parse_ns_rect("garbage"), None);
    }

    #[test]
    fn place_reuses_saved_frame_whose_middle_is_on_screen() {
        // Swift keeps a frame hanging off the edge while its middle is on.
        let visible = TerminalFrame::new(0.0, 0.0, 1920.0, 1080.0);
        let saved = TerminalFrame::new(1500.0, 100.0, 800.0, 500.0);
        assert_eq!(TerminalPanel::place(Some(saved), visible), saved);
    }

    #[test]
    fn place_reuses_saved_frame_when_it_fits() {
        let visible = TerminalFrame::new(0.0, 0.0, 1920.0, 1080.0);
        let saved = TerminalFrame::new(100.0, 100.0, 800.0, 500.0);
        assert_eq!(TerminalPanel::place(Some(saved), visible), saved);
    }

    #[test]
    fn place_clamps_offscreen_saved_frame() {
        let visible = TerminalFrame::new(0.0, 0.0, 1920.0, 1080.0);
        let saved = TerminalFrame::new(1900.0, 1000.0, 400.0, 300.0);
        let placed = TerminalPanel::place(Some(saved), visible);
        assert!(placed.x >= visible.x && placed.x + placed.width <= visible.x + visible.width);
        assert!(placed.y >= visible.y && placed.y + placed.height <= visible.y + visible.height);
    }

    #[test]
    fn place_centers_default_when_unsaved() {
        let visible = TerminalFrame::new(0.0, 0.0, 1000.0, 800.0);
        let placed = TerminalPanel::place(None, visible);
        assert_eq!(placed.width, DEFAULT_FRAME_WIDTH);
        assert_eq!(placed.width, placed.width.min(visible.width));
        assert_eq!(placed.x, (1000.0 - DEFAULT_FRAME_WIDTH) / 2.0);
    }

    #[test]
    fn test_state_json_shape() {
        let mut p = TerminalPanel::new(TerminalConfig::default());
        p.show();
        p.handle_event("size", "80x24");
        let v = p.test_state();
        assert_eq!(v["shown"], true);
        assert_eq!(v["running"], false);
        assert_eq!(v["exitCode"], -1);
        assert_eq!(v["cols"], 80);
        assert_eq!(v["rows"], 24);
        assert_eq!(v["frame"], json!([0, 0, 760, 460]));
    }
}
