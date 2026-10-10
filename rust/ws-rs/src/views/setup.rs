//! Port of `SetupWindow.swift` (`SetupWindow` + `AppInstall`) and the
//! `pylib/setup_checks.py` models it drives.
//!
//! The preflight JSON model, the Fix-action mapping, the summary line, the
//! row layout and the `AppInstall.ensureHome` marker check are complete and
//! tested; [`SetupWindow::build`] constructs the AppKit window, its rows and
//! buttons on macOS.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::app::paths::Paths;
use crate::app::process_run::run_process;
use crate::ui::chrome::Rect;
use crate::ui::theme::PopupTone;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;

// ------------------------------------------------------------------ AppInstall

/// `AppInstall.State`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    Repo,
    Ready { fresh: bool },
    Checkout,
    NotInstalled,
    Failed(String),
}

/// `AppInstall` — the home-setup state machine (no AppKit).
#[derive(Clone, Debug)]
pub struct AppInstall {
    pub state: InstallState,
    pub is_repo_build: bool,
    pub app_bundle_path: Option<String>,
    pub home_dir: String,
    pub asset_dir: String,
    pub version: String,
    pub runs_from_image: bool,
    pub keep_checkout: bool,
}

fn runs_from_image(bundle: Option<&str>) -> bool {
    match bundle {
        Some(b) => b.contains("/AppTranslocation/") || b.starts_with("/Volumes/"),
        None => false,
    }
}

impl AppInstall {
    pub fn new(
        is_repo_build: bool,
        app_bundle_path: Option<String>,
        home_dir: impl Into<String>,
        asset_dir: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        let runs = runs_from_image(app_bundle_path.as_deref());
        AppInstall {
            state: InstallState::Repo,
            is_repo_build,
            app_bundle_path,
            home_dir: home_dir.into(),
            asset_dir: asset_dir.into(),
            version: version.into(),
            runs_from_image: runs,
            keep_checkout: false,
        }
    }

    pub fn from_paths(paths: &Paths, version: impl Into<String>) -> Self {
        AppInstall::new(
            paths.is_repo_build(),
            paths.app_bundle_path.clone(),
            paths.home_dir().to_string(),
            paths.asset_dir().to_string(),
            version,
        )
    }

    pub fn marker_path(&self) -> String {
        format!("{}/.install", self.home_dir)
    }

    pub fn marker(&self) -> BTreeMap<String, String> {
        match std::fs::read_to_string(self.marker_path()) {
            Ok(text) => marker_from_text(&text),
            Err(_) => BTreeMap::new(),
        }
    }

    /// `ensureHome(switchFromCheckout:)`. Runs `bin/setup-home.sh` through bash
    /// only when the marker does not already prove the home is ready.
    pub fn ensure_home(&mut self, switch_from_checkout: bool) {
        if !self.is_repo_build {
            if let Some(bundle) = self.app_bundle_path.clone() {
                if self.runs_from_image {
                    self.state = InstallState::NotInstalled;
                    return;
                }
                let m = self.marker();
                let commands = format!("{}/commands.toml", self.home_dir);
                let app_link = format!("{}/kitchen-sink.app", self.home_dir);
                let linked = std::fs::read_link(&app_link)
                    .map(|p| p.to_string_lossy().into_owned())
                    .ok()
                    == Some(bundle.clone());
                if !switch_from_checkout
                    && m.get("mode").map(String::as_str) == Some("app")
                    && m.get("app") == Some(&bundle)
                    && m.get("version") == Some(&self.version)
                    && std::path::Path::new(&commands).exists()
                    && linked
                {
                    self.state = InstallState::Ready { fresh: false };
                    return;
                }
                let script = format!("{}/bin/setup-home.sh", self.asset_dir);
                let mut args = vec!["app".to_string(), bundle];
                if switch_from_checkout {
                    args.push("--switch".to_string());
                }
                let (code, out) = run_sync(&script, &args);
                match code {
                    0 => self.state = InstallState::Ready { fresh: true },
                    3 => self.state = InstallState::Checkout,
                    _ => {
                        let why = out
                            .lines()
                            .last()
                            .map(str::to_string)
                            .unwrap_or_else(|| "setup-home.sh failed".to_string());
                        self.state = InstallState::Failed(why);
                    }
                }
                return;
            }
        }
        self.state = InstallState::Repo;
    }

    pub fn wants_setup_window(&self) -> bool {
        match &self.state {
            InstallState::Repo => false,
            InstallState::Ready { fresh } => *fresh,
            InstallState::Checkout => !self.keep_checkout,
            InstallState::NotInstalled | InstallState::Failed(_) => true,
        }
    }
}

/// Parse the `.install` marker (`key=value`; `seed …` lines are skipped).
pub fn marker_from_text(text: &str) -> BTreeMap<String, String> {
    let mut d = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with("seed ") {
            continue;
        }
        if let Some(eq) = line.find('=') {
            d.insert(line[..eq].to_string(), line[eq + 1..].to_string());
        }
    }
    d
}

fn run_sync(script: &str, args: &[String]) -> (i32, String) {
    match run_process("/bin/bash", &[vec![script.to_string()], args.to_vec()].concat(), None, None, true) {
        Ok(r) => (r.code, r.out),
        Err(e) => (-1, format!("cannot run {script}: {e}")),
    }
}

// ---------------------------------------------------------------- check models

/// `SetupCheck` (the shape `setup_checks.parse_checks` returns).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupCheck {
    pub id: String,
    pub group: String,
    pub level: String,
    pub title: String,
    pub detail: String,
    pub fix: String,
    pub action: String,
    pub ok: bool,
}

impl SetupCheck {
    pub fn required(&self) -> bool {
        self.level == "required"
    }

    pub fn from_json(d: &Value) -> Self {
        let s = |k: &str| -> String {
            d.get(k).and_then(Value::as_str).unwrap_or("").to_string()
        };
        SetupCheck {
            id: s("id"),
            group: s("group"),
            level: s("level"),
            title: s("title"),
            detail: s("detail"),
            fix: s("fix"),
            action: s("action"),
            ok: d.get("ok") == Some(&Value::Bool(true)),
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id, "group": self.group, "level": self.level,
            "title": self.title, "detail": self.detail,
            "fix": self.fix, "action": self.action, "ok": self.ok,
        })
    }
}

/// `setup_checks.parse_checks` — non-dict entries are dropped, every field is
/// forced to a string and `ok` must be exactly `true`.
pub fn parse_checks(raw: &Value) -> Vec<SetupCheck> {
    raw.as_array()
        .map(|a| a.iter().filter(|d| d.is_object()).map(SetupCheck::from_json).collect())
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupSummary {
    pub message: String,
    pub tone: PopupTone,
}

/// `setup_checks.summarize`.
pub fn summarize(checks: &[SetupCheck]) -> SetupSummary {
    if let Some(f) = checks.iter().find(|c| !c.ok && c.required()) {
        return SetupSummary {
            message: format!("{}: {}", f.title, f.detail),
            tone: PopupTone::Danger,
        };
    }
    let warned: Vec<&SetupCheck> = checks
        .iter()
        .filter(|c| !c.ok && !c.required() && c.group != "stack")
        .collect();
    if !warned.is_empty() {
        let n = warned.len();
        return SetupSummary {
            message: format!(
                "Ready. {n} optional feature{} off — see the list.",
                if n == 1 { " is" } else { "s are" }
            ),
            tone: PopupTone::Warning,
        };
    }
    if checks.iter().any(|c| !c.ok && c.group == "stack") {
        return SetupSummary {
            message: "Ready. Hotkeys and window borders are not set up (optional).".to_string(),
            tone: PopupTone::Dim,
        };
    }
    SetupSummary {
        message: "Everything is in place.".to_string(),
        tone: PopupTone::Success,
    }
}

/// `SetupWindow.fixTitle`.
pub fn fix_title(action: &str) -> &'static str {
    if action == "move-app" {
        "Move to Applications"
    } else if action == "setup-home" {
        "Choose…"
    } else if action == "stack" {
        "Link"
    } else if action.starts_with("brew:") || action.starts_with("cask:") {
        "Install"
    } else if action.starts_with("url:") {
        "Open"
    } else if action.starts_with("term:") {
        "Copy Command"
    } else {
        "Fix"
    }
}

/// `SetupWindow.fixClicked` — the action an `action:` string maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixAction {
    MoveApp,
    SetupHome,
    Stack,
    Brew(String),
    Cask(String),
    Url(String),
    Term(String),
}

impl FixAction {
    pub fn from_action(action: &str) -> Option<FixAction> {
        if action == "move-app" {
            Some(FixAction::MoveApp)
        } else if action == "setup-home" {
            Some(FixAction::SetupHome)
        } else if action == "stack" {
            Some(FixAction::Stack)
        } else if let Some(n) = action.strip_prefix("brew:") {
            Some(FixAction::Brew(n.to_string()))
        } else if let Some(n) = action.strip_prefix("cask:") {
            Some(FixAction::Cask(n.to_string()))
        } else if let Some(u) = action.strip_prefix("url:") {
            Some(FixAction::Url(u.to_string()))
        } else if let Some(c) = action.strip_prefix("term:") {
            Some(FixAction::Term(c.to_string()))
        } else {
            None
        }
    }

    /// The command a runnable fix executes. `None` for the window-level
    /// actions (MoveApp / SetupHome / Url / Term) the Swift handles specially.
    pub fn command(&self, asset_dir: &str, brew_path: &str) -> Option<(String, Vec<String>)> {
        match self {
            FixAction::Stack => Some((
                "/bin/bash".to_string(),
                vec![format!("{asset_dir}/bin/setup-home.sh"), "stack".to_string()],
            )),
            FixAction::Brew(n) => Some((brew_path.to_string(), vec!["install".to_string(), n.clone()])),
            FixAction::Cask(n) => Some((
                brew_path.to_string(),
                vec!["install".to_string(), "--cask".to_string(), n.clone()],
            )),
            _ => None,
        }
    }
}

/// `SetupWindow.groupTitles`.
pub fn group_title(group: &str) -> String {
    match group {
        "core" => "THIS MAC".to_string(),
        "features" => "FEATURES (OPTIONAL)".to_string(),
        "stack" => "HOTKEYS + BORDERS (OPTIONAL)".to_string(),
        "dev" => "BUILDING FROM SOURCE".to_string(),
        other => other.to_uppercase(),
    }
}

/// Any stack check still failing (`updateButtons`'s `stackMissing`).
pub fn stack_missing(checks: &[SetupCheck]) -> bool {
    checks.iter().any(|c| !c.ok && c.group == "stack")
}

pub fn stack_formulae(checks: &[SetupCheck]) -> Vec<String> {
    checks
        .iter()
        .filter(|c| !c.ok && c.action.starts_with("brew:") && c.group == "stack")
        .map(|c| c.action["brew:".len()..].to_string())
        .collect()
}

pub fn stack_casks(checks: &[SetupCheck]) -> Vec<String> {
    checks
        .iter()
        .filter(|c| !c.ok && c.action.starts_with("cask:"))
        .map(|c| c.action["cask:".len()..].to_string())
        .collect()
}

// --------------------------------------------------------------- row layout

// `rebuildRows()` geometry (flipped rows view).
pub const CONTENT_LEFT: f64 = 14.0;
pub const CHECK_LEFT: f64 = 36.0;
pub const GLYPH_LEFT: f64 = 12.0;
pub const RIGHT_INSET: f64 = 14.0;
pub const GROUP_HEADER_SPACE: f64 = 26.0;
pub const ROW_ADVANCE: f64 = 22.0;
pub const FIX_BUTTON_HEIGHT: f64 = 22.0;
pub const LOG_HEIGHT: f64 = 130.0;

/// One laid-out row (flipped, top-left origin) of the checks list.
#[derive(Clone, Debug, PartialEq)]
pub enum SetupRowKind {
    Group(String),
    Check(usize),
    FixHint(usize),
}

/// The rects `rebuildRows()` places, derived purely from the check list.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupRowLayout {
    pub kind: SetupRowKind,
    pub y: f64,
    pub height: f64,
    pub title: String,
    pub detail: String,
    pub glyph: String,
    pub glyph_tone: PopupTone,
    pub fix_title: Option<String>,
    pub fix: Option<Rect>,
    pub fix_required: bool,
}

/// `intrinsicContentSize.width` for a fix button (the pure estimate; the
/// AppKit build substitutes the real size).
pub fn fix_button_width(title: &str) -> f64 {
    (title.chars().count() as f64 * 7.0 + 20.0).max(52.0)
}

fn wrap_height(text: &str, width: f64) -> f64 {
    if text.is_empty() || width <= 1.0 {
        return 0.0;
    }
    let cpl = (width / 6.0).floor().max(1.0);
    let lines = (text.chars().count() as f64 / cpl).ceil().max(1.0);
    lines * 15.0
}

/// `rebuildRows()` — the stacked rows for `checks` at a given content width.
/// Returns the rows and the total document height.
pub fn setup_row_layouts(checks: &[SetupCheck], width: f64) -> (Vec<SetupRowLayout>, f64) {
    let mut rows = Vec::new();
    let mut y = 8.0f64;
    let mut group = String::new();
    for (i, ck) in checks.iter().enumerate() {
        if ck.group != group {
            group = ck.group.clone();
            rows.push(SetupRowLayout {
                kind: SetupRowKind::Group(group.clone()),
                y,
                height: GROUP_HEADER_SPACE,
                title: group_title(&group),
                detail: String::new(),
                glyph: String::new(),
                glyph_tone: PopupTone::Dim,
                fix_title: None,
                fix: None,
                fix_required: false,
            });
            y += GROUP_HEADER_SPACE;
        }
        let (glyph, tone) = if ck.ok {
            ("✔".to_string(), PopupTone::Success)
        } else if ck.required() {
            ("✘".to_string(), PopupTone::Danger)
        } else {
            ("!".to_string(), PopupTone::Warning)
        };
        let mut right = width - RIGHT_INSET;
        let mut fix = None;
        let mut fix_label = None;
        if !ck.ok && !ck.action.is_empty() {
            let title = fix_title(&ck.action).to_string();
            let fw = fix_button_width(&title);
            fix = Some(Rect::new(right - fw, y - 1.0, fw, FIX_BUTTON_HEIGHT));
            right -= fw + 10.0;
            fix_label = Some(title);
        }
        rows.push(SetupRowLayout {
            kind: SetupRowKind::Check(i),
            y,
            height: ROW_ADVANCE,
            title: ck.title.clone(),
            detail: ck.detail.clone(),
            glyph,
            glyph_tone: tone,
            fix_title: fix_label,
            fix,
            fix_required: ck.required(),
        });
        y += ROW_ADVANCE;
        if !ck.ok && !ck.fix.is_empty() {
            let fh = wrap_height(&ck.fix, (right - CHECK_LEFT).max(1.0));
            rows.push(SetupRowLayout {
                kind: SetupRowKind::FixHint(i),
                y,
                height: fh,
                title: String::new(),
                detail: ck.fix.clone(),
                glyph: String::new(),
                glyph_tone: PopupTone::Dim,
                fix_title: None,
                fix: None,
                fix_required: false,
            });
            y += fh + 6.0;
        }
        y += 4.0;
    }
    (rows, y + 8.0)
}

// ------------------------------------------------------------------ preflight

/// The `preflight.sh --json` document.
#[derive(Clone, Debug, PartialEq)]
pub struct Preflight {
    pub mode: String,
    pub app: String,
    pub home: String,
    pub version: String,
    pub ok: bool,
    pub checks: Vec<SetupCheck>,
}

impl Preflight {
    pub fn from_json(obj: &Value) -> Option<Preflight> {
        let s = |k: &str| -> String { obj.get(k).and_then(Value::as_str).unwrap_or("").to_string() };
        Some(Preflight {
            mode: s("mode"),
            app: s("app"),
            home: s("home"),
            version: s("version"),
            ok: obj.get("ok") == Some(&Value::Bool(true)),
            checks: parse_checks(obj.get("checks").unwrap_or(&Value::Null)),
        })
    }
}

/// The window reads the LAST line of stdout that starts with `{`.
pub fn parse_preflight_output(out: &str) -> Option<Preflight> {
    let line = out.lines().rev().find(|l| l.starts_with('{'))?;
    let obj: Value = serde_json::from_str(line).ok()?;
    Preflight::from_json(&obj)
}

pub fn preflight_args(mode: &str, app: Option<&str>) -> Vec<String> {
    let mut args = vec!["--json".to_string(), "--mode".to_string(), mode.to_string()];
    if let Some(a) = app {
        args.push("--app".to_string());
        args.push(a.to_string());
    }
    args
}

/// `SetupWindow.runChecks` — run `bin/preflight.sh` and parse its JSON.
pub fn run_preflight(asset_dir: &str, mode: &str, app: Option<&str>) -> Option<Preflight> {
    let script = format!("{asset_dir}/bin/preflight.sh");
    let args = preflight_args(mode, app);
    let r = run_process("/bin/bash", &[vec![script], args].concat(), None, None, true).ok()?;
    parse_preflight_output(&r.out)
}

/// The fallback intro/status text (`updateIntro`), used until the host wires
/// the `[setup]` strings in.
pub const DEFAULT_INTRO: &str = "What this Mac needs. Notes, files, Jira, Confluence and AI work on their own; the Hyper hotkeys and window borders are an optional extra step.";
pub const DEFAULT_TITLE: &str = "kitchen-sink — Setup & Health Check";
pub const DEFAULT_WIDTH: f64 = 640.0;
pub const DEFAULT_HEIGHT: f64 = 620.0;

/// `SetupWindow` — the model behind the view.
#[derive(Default)]
pub struct SetupWindow {
    pub checks: Vec<SetupCheck>,
    pub busy: bool,
    pub summary: Option<SetupSummary>,
    pub log: String,
    pub shown: bool,
    pub on_recheck: Option<Box<dyn FnMut()>>,
    pub on_stack: Option<Box<dyn FnMut()>>,
    pub on_fix: Option<Box<dyn FnMut(usize)>>,
    #[cfg(target_os = "macos")]
    pub window: Option<Retained<NSWindow>>,
    #[cfg(target_os = "macos")]
    handler: Option<Retained<macos::SetupHandler>>,
}

impl std::fmt::Debug for SetupWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupWindow")
            .field("checks", &self.checks)
            .field("busy", &self.busy)
            .field("summary", &self.summary)
            .field("log", &self.log)
            .field("shown", &self.shown)
            .finish()
    }
}

impl SetupWindow {
    pub fn new() -> Self {
        SetupWindow::default()
    }

    pub fn set_checks(&mut self, checks: Vec<SetupCheck>) {
        self.summary = Some(summarize(&checks));
        self.checks = checks;
        #[cfg(target_os = "macos")]
        self.refresh_macos();
    }

    /// `recheck.isEnabled` / `done.isEnabled`.
    pub fn can_recheck(&self) -> bool {
        !self.busy
    }

    /// The "Set Up Hotkeys & Borders…" button's enabled/hidden state.
    pub fn can_stack(&self, install: &AppInstall) -> bool {
        !self.busy && stack_missing(&self.checks) && install.state != InstallState::NotInstalled
    }

    pub fn append_log(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.log.push_str(s);
        #[cfg(target_os = "macos")]
        self.append_log_macos(s);
    }

    pub fn show(&mut self) {
        self.shown = true;
    }

    pub fn close(&mut self) {
        self.shown = false;
        #[cfg(target_os = "macos")]
        if let Some(w) = &self.window {
            w.orderOut(None);
        }
    }

    /// Build the AppKit window: the scrollable checks list, the hidden log, the
    /// status line and the recheck / stack / done buttons.
    pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.build_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
    }

    #[cfg(target_os = "macos")]
    fn build_macos(&mut self, mtm: objc2::MainThreadMarker) {
        if self.window.is_some() {
            return;
        }
        let (window, handler) = macos::build_window(
            mtm,
            &self.checks,
            self.on_recheck.take(),
            self.on_stack.take(),
            self.on_fix.take(),
        );
        window.makeKeyAndOrderFront(None);
        self.window = Some(window);
        self.handler = Some(handler);
        if let Some(summary) = &self.summary {
            if let Some(h) = &self.handler {
                h.say(&summary.message, summary.tone);
            }
        }
    }

    /// `rebuildRows()` + `updateButtons()` from the model's current checks.
    #[cfg(target_os = "macos")]
    fn refresh_macos(&mut self) {
        let Some(handler) = &self.handler else {
            return;
        };
        handler.set_checks(self.checks.clone());
        handler.rebuild();
        handler.update_buttons();
        if let Some(summary) = &self.summary {
            handler.say(&summary.message, summary.tone);
        }
    }

    #[cfg(target_os = "macos")]
    fn append_log_macos(&mut self, s: &str) {
        if let Some(handler) = &self.handler {
            handler.append_log(s);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::ui::theme::{PopupColors, PopupThemeDefaults};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSFont, NSPopUpMenuWindowLevel,
        NSScrollView, NSStandardKeyBindingResponding, NSTextField, NSTextView, NSView, NSWindow,
        NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{
        NSMutableAttributedString, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString,
    };
    use std::cell::RefCell;

    const FONT_KEY: &str = "NSFont";
    const COLOR_KEY: &str = "NSColor";
    const PARA_KEY: &str = "NSParagraphStyle";

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn label(mtm: MainThreadMarker, s: &str, size: f64, color: crate::ui::theme::Rgba) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(s), mtm);
        l.setFont(Some(&NSFont::systemFontOfSize(size)));
        l.setTextColor(Some(&color.to_nscolor()));
        l
    }

    fn attributed(parts: &[(String, f64, crate::ui::theme::Rgba)]) -> Retained<objc2_foundation::NSAttributedString> {
        let acc = NSMutableAttributedString::new();
        for (text, size, color) in parts {
            let piece = NSMutableAttributedString::initWithString(
                NSMutableAttributedString::alloc(),
                &NSString::from_str(text),
            );
            let range = NSRange::new(0, piece.length());
            let font = NSFont::systemFontOfSize(*size);
            let color_ns = color.to_nscolor();
            let font_key = NSString::from_str(FONT_KEY);
            let color_key = NSString::from_str(COLOR_KEY);
            let para_key = NSString::from_str(PARA_KEY);
            let style = objc2_app_kit::NSMutableParagraphStyle::new();
            unsafe {
                piece.addAttribute_value_range(&font_key, as_any(&*font), range);
                piece.addAttribute_value_range(&color_key, as_any(&*color_ns), range);
                piece.addAttribute_value_range(&para_key, as_any(&*style), range);
            }
            acc.appendAttributedString(&piece);
        }
        acc.into_super()
    }

    pub struct FlippedViewIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSetupFlippedView"]
        #[ivars = FlippedViewIvars]
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
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FlippedViewIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    pub struct SetupHandlerIvars {
        pub checks: RefCell<Vec<SetupCheck>>,
        pub window: Retained<NSWindow>,
        pub rows_view: Retained<NSView>,
        pub status: Retained<NSTextField>,
        pub log_view: Retained<NSTextView>,
        pub stack: RefCell<Retained<NSButton>>,
        pub colors: PopupColors,
        pub on_recheck: RefCell<Option<Box<dyn FnMut()>>>,
        pub on_stack: RefCell<Option<Box<dyn FnMut()>>>,
        pub on_fix: RefCell<Option<Box<dyn FnMut(usize)>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSetupHandler"]
        #[ivars = SetupHandlerIvars]
        pub struct SetupHandler;

        impl SetupHandler {
            #[unsafe(method(recheck:))]
            fn recheck(&self, _sender: Option<&AnyObject>) {
                if let Some(cb) = self.ivars().on_recheck.borrow_mut().as_mut() {
                    cb();
                }
            }

            #[unsafe(method(stack:))]
            fn stack(&self, _sender: Option<&AnyObject>) {
                if let Some(cb) = self.ivars().on_stack.borrow_mut().as_mut() {
                    cb();
                }
            }

            #[unsafe(method(done:))]
            fn done(&self, _sender: Option<&AnyObject>) {
                self.ivars().window.close();
            }

            #[unsafe(method(fix:))]
            fn fix(&self, sender: Option<&AnyObject>) {
                let tag: Option<objc2_foundation::NSInteger> = sender.map(|s| unsafe {
                    msg_send![as_any(s), tag]
                });
                if let Some(i) = tag {
                    if let Some(cb) = self.ivars().on_fix.borrow_mut().as_mut() {
                        cb(i as usize);
                    }
                }
            }
        }

        unsafe impl NSObjectProtocol for SetupHandler {}
    );

    impl SetupHandler {
        pub fn set_checks(&self, checks: Vec<SetupCheck>) {
            *self.ivars().checks.borrow_mut() = checks;
        }

        /// `say(_:tone:)`.
        pub fn say(&self, s: &str, tone: PopupTone) {
            let status = &self.ivars().status;
            status.setStringValue(&NSString::from_str(s));
            status.setTextColor(Some(&self.ivars().colors.tone(tone).to_nscolor()));
        }

        /// The real "Set Up Hotkeys & Borders…" button (built after the handler
        /// because it needs the handler as its target).
        pub fn set_stack(&self, button: Retained<NSButton>) {
            *self.ivars().stack.borrow_mut() = button;
        }

        /// `updateButtons()`.
        pub fn update_buttons(&self) {
            let missing = stack_missing(&self.ivars().checks.borrow());
            let stack = self.ivars().stack.borrow();
            stack.setEnabled(missing);
            stack.setHidden(!missing);
        }

        /// `rebuildRows()`.
        pub fn rebuild(&self) {
            let mtm = MainThreadMarker::new().expect("setup rebuild on the main thread");
            let rows_view = &self.ivars().rows_view;
            let subs = rows_view.subviews();
            for s in &subs {
                s.removeFromSuperview();
            }
            let width = rows_view.bounds().size.width.max(1.0);
            let checks = self.ivars().checks.borrow();
            let (rows, total) = setup_row_layouts(&checks, width);
            let colors = self.ivars().colors;
            for row in &rows {
                match &row.kind {
                    SetupRowKind::Group(_) => {
                        let head = label(mtm, &row.title, 10.0, colors.dim);
                        head.setFont(Some(&NSFont::boldSystemFontOfSize(10.0)));
                        head.setFrame(NSRect::new(
                            NSPoint::new(CONTENT_LEFT, row.y + 6.0),
                            NSSize::new(width - CONTENT_LEFT * 2.0, 14.0),
                        ));
                        rows_view.addSubview(&head);
                    }
                    SetupRowKind::Check(_) => {
                        let glyph = label(mtm, &row.glyph, 13.0, colors.tone(row.glyph_tone));
                        glyph.setAlignment(objc2_app_kit::NSTextAlignment::Center);
                        glyph.setFrame(NSRect::new(
                            NSPoint::new(GLYPH_LEFT, row.y + 1.0),
                            NSSize::new(18.0, 18.0),
                        ));
                        rows_view.addSubview(&glyph);

                        let mut right = width - RIGHT_INSET;
                        if let Some(title) = &row.fix_title {
                            let b = unsafe {
                                NSButton::buttonWithTitle_target_action(
                                    &NSString::from_str(title),
                                    Some(as_any(self)),
                                    Some(objc2::sel!(fix:)),
                                    mtm,
                                )
                            };
                            b.setTag(match &row.kind {
                                SetupRowKind::Check(i) => *i as objc2_foundation::NSInteger,
                                _ => 0,
                            });
                            b.sizeToFit();
                            let fw = b.frame().size.width;
                            b.setFrame(NSRect::new(
                                NSPoint::new(width - RIGHT_INSET - fw, row.y - 1.0),
                                NSSize::new(fw, FIX_BUTTON_HEIGHT),
                            ));
                            right -= fw + 10.0;
                            rows_view.addSubview(&b);
                        }

                        let parts = vec![
                            (row.title.clone(), 12.0, colors.text),
                            (
                                if row.detail.is_empty() {
                                    String::new()
                                } else {
                                    format!("   {}", row.detail)
                                },
                                11.0,
                                colors.dim,
                            ),
                        ];
                        let line = NSTextField::labelWithAttributedString(&attributed(&parts), mtm);
                        line.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                        line.setFrame(NSRect::new(
                            NSPoint::new(CHECK_LEFT, row.y + 2.0),
                            NSSize::new((right - CHECK_LEFT).max(1.0), 17.0),
                        ));
                        line.setToolTip(Some(&NSString::from_str(&row.detail)));
                        rows_view.addSubview(&line);
                    }
                    SetupRowKind::FixHint(_) => {
                        let hint = NSTextField::wrappingLabelWithString(&NSString::from_str(&row.detail), mtm);
                        hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                        hint.setTextColor(Some(&colors.dim.to_nscolor()));
                        hint.setSelectable(true);
                        hint.setFrame(NSRect::new(
                            NSPoint::new(CHECK_LEFT, row.y),
                            NSSize::new((width - RIGHT_INSET - CHECK_LEFT).max(1.0), row.height.max(1.0)),
                        ));
                        rows_view.addSubview(&hint);
                    }
                }
            }
            rows_view.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, total),
            ));
        }

        /// `appendLog(_:)`.
        pub fn append_log(&self, s: &str) {
            if s.is_empty() {
                return;
            }
            if let Some(store) = unsafe { self.ivars().log_view.textStorage() } {
                store.appendAttributedString(&attributed(&[(s.to_string(), 11.0, self.ivars().colors.dim)]));
            }
            unsafe { self.ivars().log_view.scrollToEndOfDocument(None) };
        }
    }

    thread_local! {
        static LIVE_HANDLERS: RefCell<Vec<Retained<SetupHandler>>>
            = const { RefCell::new(Vec::new()) };
    }

    /// The window + its handler, wired to the model's callbacks.
    pub fn build_window(
        mtm: MainThreadMarker,
        checks: &[SetupCheck],
        on_recheck: Option<Box<dyn FnMut()>>,
        on_stack: Option<Box<dyn FnMut()>>,
        on_fix: Option<Box<dyn FnMut(usize)>>,
    ) -> (Retained<NSWindow>, Retained<SetupHandler>) {
        let colors = PopupThemeDefaults::colors();
        let w = DEFAULT_WIDTH;
        let h = DEFAULT_HEIGHT;
        let mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)),
                mask,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str(DEFAULT_TITLE));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        unsafe { window.setReleasedWhenClosed(false) };
        window.setBackgroundColor(Some(&colors.base().to_nscolor()));
        let appearance = unsafe {
            if colors.is_light() {
                NSAppearanceNameAqua
            } else {
                NSAppearanceNameDarkAqua
            }
        };
        if let Some(app) = NSAppearance::appearanceNamed(appearance) {
            window.setAppearance(Some(&app));
        }
        window.setMinSize(NSSize::new(520.0, 440.0));
        window.setLevel(NSPopUpMenuWindowLevel + 1);

        let content = NSView::new(mtm);
        let rect = window.contentLayoutRect();
        content.setFrame(rect);
        content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let cw = rect.size.width;
        let ch = rect.size.height;

        let intro = NSTextField::wrappingLabelWithString(&NSString::from_str(DEFAULT_INTRO), mtm);
        intro.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        intro.setTextColor(Some(&colors.dim.to_nscolor()));
        intro.setFrame(NSRect::new(
            NSPoint::new(20.0, ch - 78.0),
            NSSize::new(cw - 40.0, 40.0),
        ));
        intro.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&intro);

        let scroll = NSScrollView::new(mtm);
        scroll.setFrame(NSRect::new(
            NSPoint::new(20.0, 64.0),
            NSSize::new(cw - 40.0, ch - 152.0),
        ));
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(true);
        scroll.setBackgroundColor(&colors.mantle().to_nscolor());
        scroll.setWantsLayer(true);
        if let Some(layer) = scroll.layer() {
            layer.setCornerRadius(8.0);
            layer.setBorderWidth(1.0);
            layer.setBorderColor(Some(&colors.hairline().to_nscolor().CGColor()));
        }
        let rows_view = FlippedView::new(mtm);
        rows_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        scroll.setDocumentView(Some(&rows_view));
        content.addSubview(&scroll);

        let log_scroll = NSScrollView::new(mtm);
        log_scroll.setFrame(NSRect::new(
            NSPoint::new(20.0, 64.0),
            NSSize::new(cw - 40.0, LOG_HEIGHT),
        ));
        log_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        log_scroll.setHasVerticalScroller(true);
        log_scroll.setDrawsBackground(true);
        log_scroll.setBackgroundColor(&colors.crust().to_nscolor());
        log_scroll.setWantsLayer(true);
        if let Some(layer) = log_scroll.layer() {
            layer.setCornerRadius(8.0);
        }
        let log_view = NSTextView::new(mtm);
        log_view.setEditable(false);
        log_view.setSelectable(true);
        log_view.setDrawsBackground(false);
        log_view.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(11.0, 0.0)));
        log_view.setTextColor(Some(&colors.dim.to_nscolor()));
        log_view.setTextContainerInset(NSSize::new(6.0, 6.0));
        log_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(log_scroll.contentSize().width, LOG_HEIGHT),
        ));
        log_scroll.setDocumentView(Some(&log_view));
        log_scroll.setHidden(true);
        content.addSubview(&log_scroll);

        let status = label(mtm, "", 11.0, colors.dim);
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 44.0),
            NSSize::new(cw - 40.0, 16.0),
        ));
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview(&status);

        let handler = SetupHandler::alloc(mtm).set_ivars(SetupHandlerIvars {
            checks: RefCell::new(checks.to_vec()),
            window: window.clone(),
            rows_view: rows_view.clone().into_super(),
            status: status.clone(),
            log_view: log_view.clone(),
            stack: RefCell::new(create_button(mtm, "Set Up Hotkeys & Borders…", None, None)),
            colors,
            on_recheck: RefCell::new(on_recheck),
            on_stack: RefCell::new(on_stack),
            on_fix: RefCell::new(on_fix),
        });
        let handler: Retained<SetupHandler> = unsafe { msg_send![super(handler), init] };
        LIVE_HANDLERS.with(|v| v.borrow_mut().push(handler.clone()));

        let recheck = create_button(
            mtm,
            "Check Again",
            Some(as_any(&*handler)),
            Some(objc2::sel!(recheck:)),
        );
        let stack = create_button(
            mtm,
            "Set Up Hotkeys & Borders…",
            Some(as_any(&*handler)),
            Some(objc2::sel!(stack:)),
        );
        let done = create_button(mtm, "Done", Some(as_any(&*handler)), Some(objc2::sel!(done:)));
        handler.set_stack(stack.clone());
        for b in [&recheck, &stack, &done] {
            b.sizeToFit();
            content.addSubview(b);
        }
        let bw = |b: &NSButton| {
            b.sizeToFit();
            b.frame().size.width
        };
        done.setFrame(NSRect::new(
            NSPoint::new(cw - 20.0 - bw(&done), 12.0),
            NSSize::new(bw(&done), 26.0),
        ));
        done.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        recheck.setFrame(NSRect::new(
            NSPoint::new(20.0, 12.0),
            NSSize::new(bw(&recheck), 26.0),
        ));
        recheck.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        stack.setFrame(NSRect::new(
            NSPoint::new(20.0 + bw(&recheck) + 8.0, 12.0),
            NSSize::new(bw(&stack), 26.0),
        ));
        stack.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        window.setContentView(Some(&content));

        handler.rebuild();
        handler.update_buttons();
        let summary = summarize(checks);
        handler.say(&summary.message, summary.tone);
        window.center();
        (window, handler)
    }

    fn create_button(
        mtm: MainThreadMarker,
        title: &str,
        target: Option<&AnyObject>,
        action: Option<objc2::runtime::Sel>,
    ) -> Retained<NSButton> {
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                target,
                action,
                mtm,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(over: Value) -> Value {
        let mut d = json!({
            "id": "python", "group": "core", "level": "required", "title": "Python",
            "detail": "python 3.11+", "fix": "brew install python", "action": "brew:python", "ok": true
        });
        if let (Some(base), Some(extra)) = (d.as_object_mut(), over.as_object()) {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }
        d
    }

    #[test]
    fn parse_fields_and_defaults() {
        let out = parse_checks(&json!([check(json!({"extra": 1})), {"title": 7}, "nope", null]));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Python");
        assert!(out[0].ok);
        assert_eq!(
            out[1],
            SetupCheck {
                id: "".into(),
                group: "".into(),
                level: "".into(),
                title: "".into(),
                detail: "".into(),
                fix: "".into(),
                action: "".into(),
                ok: false,
            }
        );
        assert_eq!(parse_checks(&Value::Null), Vec::<SetupCheck>::new());
    }

    #[test]
    fn ok_is_strict() {
        assert!(!parse_checks(&json!([check(json!({"ok": 1}))]))[0].ok);
        assert!(parse_checks(&json!([check(json!({"ok": true}))]))[0].ok);
    }

    #[test]
    fn summarize_cases() {
        let v = |o: Value| SetupCheck::from_json(&check(o));

        let s = summarize(&[
            v(json!({"ok": false, "title": "Python", "detail": "missing"})),
            v(json!({"id": "x", "group": "features", "level": "optional", "ok": false})),
        ]);
        assert_eq!(s.message, "Python: missing");
        assert_eq!(s.tone, PopupTone::Danger);

        let s = summarize(&[
            v(json!({})),
            v(json!({"id": "a", "group": "features", "level": "optional", "ok": false})),
        ]);
        assert_eq!(s.message, "Ready. 1 optional feature is off — see the list.");
        assert_eq!(s.tone, PopupTone::Warning);

        let s = summarize(&[
            v(json!({})),
            v(json!({"id": "1", "group": "features", "level": "optional", "ok": false})),
            v(json!({"id": "2", "group": "features", "level": "optional", "ok": false})),
        ]);
        assert_eq!(s.message, "Ready. 2 optional features are off — see the list.");

        let s = summarize(&[
            v(json!({})),
            v(json!({"id": "b", "group": "stack", "level": "optional", "ok": false})),
        ]);
        assert_eq!(s.tone, PopupTone::Dim);
        assert!(s.message.contains("Hotkeys"));

        assert_eq!(summarize(&[v(json!({}))]).message, "Everything is in place.");
        assert_eq!(summarize(&[]).tone, PopupTone::Success);
    }

    #[test]
    fn fix_titles() {
        assert_eq!(fix_title("move-app"), "Move to Applications");
        assert_eq!(fix_title("setup-home"), "Choose…");
        assert_eq!(fix_title("stack"), "Link");
        assert_eq!(fix_title("brew:python"), "Install");
        assert_eq!(fix_title("cask:ghostty"), "Install");
        assert_eq!(fix_title("url:https://x"), "Open");
        assert_eq!(fix_title("term:xcode-select --install"), "Copy Command");
        assert_eq!(fix_title("something"), "Fix");
    }

    #[test]
    fn fix_action_mapping() {
        assert_eq!(FixAction::from_action("move-app"), Some(FixAction::MoveApp));
        assert_eq!(FixAction::from_action("setup-home"), Some(FixAction::SetupHome));
        assert_eq!(FixAction::from_action("stack"), Some(FixAction::Stack));
        assert_eq!(FixAction::from_action("brew:python"), Some(FixAction::Brew("python".into())));
        assert_eq!(FixAction::from_action("cask:ghostty"), Some(FixAction::Cask("ghostty".into())));
        assert_eq!(
            FixAction::from_action("url:x-apple.systempreferences:x"),
            Some(FixAction::Url("x-apple.systempreferences:x".into()))
        );
        assert_eq!(
            FixAction::from_action("term:brew install x"),
            Some(FixAction::Term("brew install x".into()))
        );
        assert_eq!(FixAction::from_action("nope"), None);

        let (exe, args) = FixAction::Stack.command("/app", "/opt/homebrew/bin/brew").unwrap();
        assert_eq!(exe, "/bin/bash");
        assert_eq!(args, vec!["/app/bin/setup-home.sh", "stack"]);
        let (exe, args) = FixAction::Brew("python".into()).command("/app", "/b").unwrap();
        assert_eq!((exe.as_str(), args), ("/b", vec!["install".to_string(), "python".to_string()]));
        assert!(FixAction::MoveApp.command("/app", "/b").is_none());
    }

    #[test]
    fn marker_parses_and_skips_seed() {
        let m = marker_from_text("mode=app\napp=/Applications/kitchen-sink.app\nversion=1.2\nseed abc file\nextra=1\nbadline\n");
        assert_eq!(m.get("mode").map(String::as_str), Some("app"));
        assert_eq!(m.get("version").map(String::as_str), Some("1.2"));
        assert_eq!(m.get("extra").map(String::as_str), Some("1"));
        assert!(!m.contains_key("seed abc file"));
    }

    #[test]
    fn install_state_transitions() {
        let mut repo = AppInstall::new(true, None, "/h", "/a", "1");
        repo.ensure_home(false);
        assert_eq!(repo.state, InstallState::Repo);
        assert!(!repo.wants_setup_window());

        let mut img = AppInstall::new(
            false,
            Some("/Volumes/kitchen-sink/kitchen-sink.app".into()),
            "/h",
            "/a",
            "1",
        );
        img.ensure_home(false);
        assert_eq!(img.state, InstallState::NotInstalled);
        assert!(img.wants_setup_window());

        let fresh = AppInstall {
            state: InstallState::Ready { fresh: true },
            ..AppInstall::new(false, Some("/Applications/x.app".into()), "/h", "/a", "1")
        };
        assert!(fresh.wants_setup_window());
        let done = AppInstall {
            state: InstallState::Ready { fresh: false },
            ..AppInstall::new(false, Some("/Applications/x.app".into()), "/h", "/a", "1")
        };
        assert!(!done.wants_setup_window());

        let checkout = AppInstall {
            state: InstallState::Checkout,
            ..AppInstall::new(false, Some("/Applications/x.app".into()), "/h", "/a", "1")
        };
        assert!(checkout.wants_setup_window());
        let kept = AppInstall {
            keep_checkout: true,
            ..checkout
        };
        assert!(!kept.wants_setup_window());
    }

    #[test]
    fn preflight_output_parsing() {
        let out = "noise\nwarning line\n{\"mode\":\"app\",\"app\":\"/Applications/x.app\",\"home\":\"~/x\",\"version\":\"1.2\",\"ok\":false,\"checks\":[{\"id\":\"macos\",\"group\":\"core\",\"level\":\"required\",\"ok\":false,\"title\":\"macOS 14\",\"detail\":\"this Mac runs 13\",\"fix\":\"Update\",\"action\":\"url:x\"}]}\n";
        let p = parse_preflight_output(out).unwrap();
        assert_eq!(p.mode, "app");
        assert_eq!(p.version, "1.2");
        assert!(!p.ok);
        assert_eq!(p.checks.len(), 1);
        assert_eq!(p.checks[0].action, "url:x");
        assert!(p.checks[0].required());

        assert!(parse_preflight_output("no json here").is_none());
        assert!(parse_preflight_output("{\"mode\":").is_none());

        assert_eq!(
            preflight_args("repo", None),
            vec!["--json", "--mode", "repo"]
        );
        assert_eq!(
            preflight_args("app", Some("/Applications/x.app")),
            vec!["--json", "--mode", "app", "--app", "/Applications/x.app"]
        );
    }

    #[test]
    fn stack_helpers_and_buttons() {
        let checks = vec![
            SetupCheck::from_json(&check(json!({}))),
            SetupCheck::from_json(&check(json!({"id": "aero", "group": "stack", "level": "optional", "ok": false, "action": "brew:aerospace"}))),
            SetupCheck::from_json(&check(json!({"id": "borders", "group": "stack", "level": "optional", "ok": false, "action": "cask:borders"}))),
        ];
        assert!(stack_missing(&checks));
        assert_eq!(stack_formulae(&checks), vec!["aerospace"]);
        assert_eq!(stack_casks(&checks), vec!["borders"]);
        assert_eq!(group_title("core"), "THIS MAC");
        assert_eq!(group_title("stack"), "HOTKEYS + BORDERS (OPTIONAL)");
        assert_eq!(group_title("other"), "OTHER");

        let install = AppInstall::new(false, Some("/Applications/x.app".into()), "/h", "/a", "1");
        let mut w = SetupWindow::new();
        w.set_checks(checks);
        assert!(w.can_stack(&install));
        w.busy = true;
        assert!(!w.can_stack(&install));
        assert!(!w.can_recheck());
    }

    #[test]
    fn layout_emits_group_headers_and_rows() {
        let checks = vec![
            SetupCheck::from_json(&check(json!({}))),
            SetupCheck::from_json(&check(json!({
                "id": "aero", "group": "stack", "level": "optional", "ok": false,
                "action": "brew:aerospace", "title": "AeroSpace", "detail": "", "fix": ""
            }))),
        ];
        let (rows, total) = setup_row_layouts(&checks, 600.0);
        assert!(matches!(&rows[0].kind, SetupRowKind::Group(g) if g == "core"));
        assert_eq!(rows[0].title, "THIS MAC");
        assert_eq!(rows[0].y, 8.0);
        assert!(matches!(&rows[1].kind, SetupRowKind::Check(0)));
        assert_eq!(rows[1].glyph, "✔");
        assert_eq!(rows[1].glyph_tone, PopupTone::Success);
        // The stack group header sits after the core check row.
        let header = rows
            .iter()
            .find(|r| r.title.starts_with("HOTKEYS"))
            .expect("stack header");
        assert!(header.y > rows[1].y);
        assert!(total > header.y);
    }

    #[test]
    fn layout_failing_rows_get_fix_buttons_and_hints() {
        let checks = vec![SetupCheck::from_json(&check(json!({
            "id": "python", "group": "core", "level": "required", "ok": false,
            "title": "Python", "detail": "missing", "action": "brew:python",
            "fix": "brew install python"
        })))];
        let (rows, _total) = setup_row_layouts(&checks, 600.0);
        let check_row = rows
            .iter()
            .find(|r| matches!(r.kind, SetupRowKind::Check(0)))
            .unwrap();
        assert_eq!(check_row.glyph, "✘");
        assert_eq!(check_row.glyph_tone, PopupTone::Danger);
        let fix = check_row.fix.expect("fix button");
        assert!(fix.width >= 52.0);
        assert!(fix.x + fix.width <= 600.0 - RIGHT_INSET + 0.001);
        assert!(check_row.fix_title.is_some());
        let hint = rows
            .iter()
            .find(|r| matches!(r.kind, SetupRowKind::FixHint(0)))
            .expect("fix hint row");
        assert_eq!(hint.detail, "brew install python");
        assert!(hint.height > 0.0);
    }

    #[test]
    fn layout_optional_failure_uses_warning_glyph() {
        let checks = vec![SetupCheck::from_json(&check(json!({
            "id": "nvim", "group": "features", "level": "optional", "ok": false,
            "title": "Neovim", "detail": "", "action": "", "fix": ""
        })))];
        let (rows, _) = setup_row_layouts(&checks, 600.0);
        let check_row = rows
            .iter()
            .find(|r| matches!(r.kind, SetupRowKind::Check(0)))
            .unwrap();
        assert_eq!(check_row.glyph, "!");
        assert_eq!(check_row.glyph_tone, PopupTone::Warning);
        assert!(check_row.fix.is_none(), "no action -> no button");
    }

    #[test]
    fn fix_button_width_scales_with_title() {
        assert!(fix_button_width("Fix") >= 52.0);
        assert!(fix_button_width("Move to Applications") > fix_button_width("Fix"));
    }
}
