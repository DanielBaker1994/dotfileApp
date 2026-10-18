//! The Jira **Connection / Setup** form (was `JiraSetupWindow` in
//! `kitchen_sink.swift`): Site / Email / Token / Projects-in-scope /
//! Max-results, Test Connection (`jira_api.py --detect-auth`), Copy curl
//! (`jira_api.py --curl --myself`) and Save & Enable (`jira_config.py --save`
//! + `--team-set project_keys`). It is embedded **inline** in the Jira Config
//! surface (`views/jira_config.rs`, [`JiraSetupFormView`]) — there is no
//! separate window any more. The Python boundary is unchanged.
//!
//! The pure half ([`SetupValues`], [`read_detect`], [`prefill_notice`]
//! [`SetupStrings`]) is AppKit-free and unit-tested; the AppKit half lives in
//! [`appkit`]. All user-facing strings come from `commands.toml [jira-config]`
//! (rule 3).

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::app::jira_poll::{error_line, run_stdin};

// ===========================================================================
// `[jira-config]` setup-form strings (rule 3)
// ===========================================================================

/// The Connection / Setup form's user-facing strings, read from
/// `commands.toml [jira-config]` (`setup-*` keys). Defaults match the former
/// hard-coded `JiraSetupWindow`. `{name}` markers are filled by [`fill`].
#[derive(Clone, Debug, PartialEq)]
pub struct SetupStrings {
    pub section_title: String,
    pub intro: String,
    pub site_label: String,
    pub site_placeholder: String,
    pub email_label: String,
    pub email_placeholder: String,
    pub token_label: String,
    pub token_placeholder: String,
    pub token_kept: String,
    pub projects_label: String,
    pub projects_placeholder: String,
    pub max_label: String,
    pub max_placeholder: String,
    pub max_default: i64,
    pub test_label: String,
    pub curl_label: String,
    pub curl_tooltip: String,
    pub save_label: String,
    pub auth_basic: String,
    pub auth_bearer: String,
    pub testing: String,
    pub detecting: String,
    pub connected: String,
    pub test_failed: String,
    pub detect_hint: String,
    pub site_required: String,
    pub bad_keys: String,
    pub need_project: String,
    pub save_failed: String,
    pub projects_failed: String,
    pub login_failed: String,
    pub enabling: String,
    pub curl_copied: String,
    pub curl_failed: String,
    pub saves_to: String,
    pub env_note: String,
}

impl Default for SetupStrings {
    fn default() -> Self {
        SetupStrings {
            section_title: "Connection / Setup".to_string(),
            intro: "Paste your token — the auth type is detected on Test / Save. Server / Data Center personal access tokens go out as Authorization: Bearer; a Jira Cloud API token also needs the account email.".to_string(),
            site_label: "Site URL".to_string(),
            site_placeholder: "https://jira.example.com".to_string(),
            email_label: "Email (Cloud only)".to_string(),
            email_placeholder: "only for *.atlassian.net — blank for a personal access token".to_string(),
            token_label: "Token".to_string(),
            token_placeholder: "personal access token / API token".to_string(),
            token_kept: "token set — leave blank to keep it".to_string(),
            projects_label: "Projects in scope".to_string(),
            projects_placeholder: "e.g. SAM1, KAN — required; every query stays inside these".to_string(),
            max_label: "Max results".to_string(),
            max_placeholder: "25".to_string(),
            max_default: 25,
            test_label: "Test Connection".to_string(),
            curl_label: "Copy curl".to_string(),
            curl_tooltip: "Copy the login test (GET /rest/api/2/myself) as a curl command".to_string(),
            save_label: "Save & Enable".to_string(),
            auth_basic: "email + API token (Cloud)".to_string(),
            auth_bearer: "Bearer token (Server / Data Center)".to_string(),
            testing: "Testing (detecting the auth type)…".to_string(),
            detecting: "Detecting the auth type…".to_string(),
            connected: "✓ Connected as {user} — {auth}".to_string(),
            test_failed: "✗ {error}".to_string(),
            detect_hint: " — Copy curl to reproduce in a terminal".to_string(),
            site_required: "✗ Site URL must start with https://".to_string(),
            bad_keys: "✗ Not a project key: {keys} — use keys like SAM1, KAN".to_string(),
            need_project: "✗ Enter at least one project in scope — every query is limited to these projects".to_string(),
            save_failed: "✗ save failed: {error}".to_string(),
            projects_failed: "✗ projects in scope not saved: {error}".to_string(),
            login_failed: "✗ saved, but login failed: {error} — polling stays off".to_string(),
            enabling: "✓ Connected as {user} — {auth} — enabling…".to_string(),
            curl_copied: "curl copied (includes the token) — paste into a terminal to test the login.".to_string(),
            curl_failed: "✗ {error}".to_string(),
            saves_to: "Saves to {path} (chmod 600).".to_string(),
            env_note: "Using JIRA_{key} from the environment — save will persist it into config.json.".to_string(),
        }
    }
}

impl SetupStrings {
    /// Read `[jira-config]`; defaults on failure.
    pub fn load() -> SetupStrings {
        match crate::app::config::read_config_text() {
            Some(text) => SetupStrings::from_text(&text),
            None => SetupStrings::default(),
        }
    }

    pub fn from_text(text: &str) -> SetupStrings {
        SetupStrings::from_section(&crate::views::jira_config::parse_section(text, "jira-config"))
    }

    /// The pure key parsing (`load()` without the file read).
    pub fn from_section(e: &HashMap<String, String>) -> SetupStrings {
        let d = SetupStrings::default();
        let s = |k: &str, dv: &str| match e.get(k) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => dv.to_string(),
        };
        SetupStrings {
            section_title: s("setup-section-title", &d.section_title),
            intro: s("setup-intro", &d.intro),
            site_label: s("setup-site-label", &d.site_label),
            site_placeholder: s("setup-site-placeholder", &d.site_placeholder),
            email_label: s("setup-email-label", &d.email_label),
            email_placeholder: s("setup-email-placeholder", &d.email_placeholder),
            token_label: s("setup-token-label", &d.token_label),
            token_placeholder: s("setup-token-placeholder", &d.token_placeholder),
            token_kept: s("setup-token-kept", &d.token_kept),
            projects_label: s("setup-projects-label", &d.projects_label),
            projects_placeholder: s("setup-projects-placeholder", &d.projects_placeholder),
            max_label: s("setup-max-label", &d.max_label),
            max_placeholder: s("setup-max-placeholder", &d.max_placeholder),
            max_default: e
                .get("setup-max-default")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(d.max_default),
            test_label: s("setup-test-label", &d.test_label),
            curl_label: s("setup-curl-label", &d.curl_label),
            curl_tooltip: s("setup-curl-tooltip", &d.curl_tooltip),
            save_label: s("setup-save-label", &d.save_label),
            auth_basic: s("setup-auth-basic", &d.auth_basic),
            auth_bearer: s("setup-auth-bearer", &d.auth_bearer),
            testing: s("setup-testing", &d.testing),
            detecting: s("setup-detecting", &d.detecting),
            connected: s("setup-connected", &d.connected),
            test_failed: s("setup-test-failed", &d.test_failed),
            detect_hint: s("setup-detect-hint", &d.detect_hint),
            site_required: s("setup-site-required", &d.site_required),
            bad_keys: s("setup-bad-keys", &d.bad_keys),
            need_project: s("setup-need-project", &d.need_project),
            save_failed: s("setup-save-failed", &d.save_failed),
            projects_failed: s("setup-projects-failed", &d.projects_failed),
            login_failed: s("setup-login-failed", &d.login_failed),
            enabling: s("setup-enabling", &d.enabling),
            curl_copied: s("setup-curl-copied", &d.curl_copied),
            curl_failed: s("setup-curl-failed", &d.curl_failed),
            saves_to: s("setup-saves-to", &d.saves_to),
            env_note: s("setup-env-note", &d.env_note),
        }
    }
}

/// `{name}` → value substitution for the `[jira-config]` format strings.
pub fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut s = template.to_string();
    for (k, v) in pairs {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// `authTitle(_:)`: the detected auth mode's human name.
pub fn auth_title<'a>(s: &'a SetupStrings, mode: &str) -> &'a str {
    if mode == "basic" {
        &s.auth_basic
    } else {
        &s.auth_bearer
    }
}

// ===========================================================================
// Pure model (AppKit-free, unit-tested)
// ===========================================================================

/// The typed values, trimmed (`trimmed`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupValues {
    pub site: String,
    pub email: String,
    pub token: String,
    pub project: String,
    pub max: String,
}

impl SetupValues {
    /// `typedArgs()`: the shared `--email/--site/--token-stdin` arguments.
    pub fn typed_args(&self) -> (Vec<String>, Option<String>) {
        let mut args = vec!["--email".to_string(), self.email.clone()];
        if !self.site.is_empty() {
            args.extend(["--site".to_string(), self.site.clone()]);
        }
        let stdin = (!self.token.is_empty()).then(|| {
            args.push("--token-stdin".into());
            format!("{}\n", self.token)
        });
        (args, stdin)
    }

    /// The `--save` object (`saveAndEnable`).
    pub fn save_object(&self, mode: &str, keys: &[String], max_default: i64) -> Value {
        let mut o = json!({
            "site": self.site,
            "email": if mode == "basic" { self.email.as_str() } else { "" },
            "auth": mode,
            "defaultProject": keys.first().cloned().unwrap_or_default(),
            "defaultMax": self.max.parse::<i64>().unwrap_or(max_default),
        });
        if !self.token.is_empty() {
            o["token"] = json!(self.token);
        }
        o
    }
}

/// `--detect-auth`'s answer: `Ok((auth, user))` or the failure line.
pub fn read_detect(code: i32, out: &str, err: &str, hint: &str) -> Result<(String, String), String> {
    let d: Value = serde_json::from_str(out).unwrap_or(Value::Null);
    if code == 0 {
        if let Some(auth) = d.get("auth").and_then(Value::as_str) {
            let user = d.get("user").and_then(Value::as_str).unwrap_or("unknown user");
            return Ok((auth.to_string(), user.to_string()));
        }
    }
    let tried: Vec<String> = d
        .get("tried")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|t| {
                    let mode = t.get("mode").and_then(Value::as_str).unwrap_or("?");
                    let http = t.get("http").and_then(Value::as_i64).map(|h| h.to_string()).unwrap_or("?".into());
                    format!("{mode} → HTTP {http}")
                })
                .collect()
        })
        .unwrap_or_default();
    let msg = d
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error_line(err, &format!("login failed (exit {code})")));
    let tried = if tried.is_empty() { String::new() } else { format!(" (tried {})", tried.join(", ")) };
    Err(format!("{msg}{tried}{hint}"))
}

/// `detect(done:)`, blocking.
pub fn detect(v: &SetupValues, hint: &str) -> Result<(String, String), String> {
    let (args, stdin) = v.typed_args();
    let mut all = vec!["--detect-auth".to_string()];
    all.extend(args);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let (code, out, err) = run_stdin("jira_api.py", &refs, stdin.as_deref());
    read_detect(code, &out, &err, hint)
}

/// `parseProjectKeys(_:)` through the helper (`jira.project_keys`). A helper
/// failure used to answer (empty, empty): keys the user had typed simply
/// vanished. The grammar is tiny — port it as the fallback (same rules as
/// pylib/jira_setup.py).
pub fn parse_project_keys(raw: &str) -> (Vec<String>, Vec<String>) {
    let list = |v: &Value, k: &str| -> Vec<String> {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    match crate::app::python_helper::PythonHelper::shared().call(
        "jira.project_keys",
        json!({ "raw": raw }),
        std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(5),
    ) {
        Ok(v) => (list(&v, "keys"), list(&v, "bad")),
        Err(_) => project_keys_native(raw),
    }
}

fn project_keys_native(raw: &str) -> (Vec<String>, Vec<String>) {
    let mut keys: Vec<String> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    let up = raw.to_uppercase();
    for part in up.split(|c: char| c == ',' || c.is_whitespace()) {
        if part.is_empty() {
            continue;
        }
        let ok = {
            let mut chars = part.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
                && part.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        };
        if !ok {
            bad.push(part.to_string());
        } else if !keys.iter().any(|k| k == part) {
            keys.push(part.to_string());
        }
    }
    (keys, bad)
}

/// `JiraPoll.configPath`.
pub fn config_path() -> String {
    std::env::var("JIRA_CONFIG_JSON").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| {
        format!("{}/.config/jira/config.json", std::env::var("HOME").unwrap_or_default())
    })
}

/// `prefill()`'s notice lines from the `--check` JSON.
pub fn prefill_notice(d: &Value, s: &SetupStrings) -> String {
    let notes: Vec<&str> = d.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut lines = vec![fill(&s.saves_to, &[("path", &config_path())])];
    for k in ["SITE", "EMAIL", "TOKEN"] {
        if notes.iter().any(|n| n.contains(&format!("env JIRA_{k}"))) {
            lines.push(fill(&s.env_note, &[("key", k)]));
        }
    }
    lines.join("\n")
}

#[cfg(target_os = "macos")]
pub use appkit::{JiraSetupFormView, SetupOutcome};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::theme::{PopupColors, PopupTone};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSFont, NSSecureTextField,
        NSTextAlignment, NSTextField, NSView,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const INNER: f64 = 4.0;
    const LABEL_W: f64 = 132.0;
    const ROW_H: f64 = 30.0;
    const FIELD_H: f64 = 24.0;
    const INTRO_H: f64 = 54.0;
    const NOTICE_H: f64 = 34.0;
    const RESULT_H: f64 = 38.0;
    const BUTTON_H: f64 = 26.0;
    /// Fixed form height (`INTRO` + 5 rows + notice + result + buttons); the
    /// Config body reserves exactly this much.
    const FORM_HEIGHT: f64 =
        INTRO_H + 6.0 + 5.0 * ROW_H + NOTICE_H + 4.0 + RESULT_H + 6.0 + BUTTON_H;

    /// What the embedded form reports to its host (the Jira Config body).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SetupOutcome {
        /// Save & Enable succeeded: the host enables polling.
        Saved,
    }

    /// A Save & Enable failure, mapped to a `[jira-config]` message by the form.
    enum SaveErr {
        BadKeys(String),
        NoProjects,
        Save(String),
        Projects(String),
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)))
    }

    use crate::ui::view_base::as_any;

    pub struct FormIvars {
        settings: SetupStrings,
        colors: PopupColors,
        intro: Retained<NSTextField>,
        row_labels: [Retained<NSTextField>; 5],
        site: Retained<NSTextField>,
        email: Retained<NSTextField>,
        token: Retained<NSTextField>,
        project: Retained<NSTextField>,
        max: Retained<NSTextField>,
        notice: Retained<NSTextField>,
        result: Retained<NSTextField>,
        /// Test Connection / Copy curl / Save & Enable.
        buttons: [Retained<crate::ui::outline_button::OutlineButton>; 3],
        /// The `--detect-auth` answer (for Copy curl's `--auth`).
        detected: RefCell<Option<String>>,
        sink: RefCell<Option<Rc<dyn Fn(SetupOutcome)>>>,
        busy: Cell<bool>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraSetupFormView"]
        #[ivars = FormIvars]
        pub struct JiraSetupFormView;

        impl JiraSetupFormView {
            /// The Config body sets the frame to the scroll width; re-lay out.
            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout();
            }

            #[unsafe(method(testClicked:))]
            fn test_clicked(&self, _s: Option<&AnyObject>) {
                self.test();
            }

            #[unsafe(method(curlClicked:))]
            fn curl_clicked(&self, _s: Option<&AnyObject>) {
                self.copy_curl();
            }

            #[unsafe(method(saveClicked:))]
            fn save_clicked(&self, _s: Option<&AnyObject>) {
                self.save();
            }
        }

        unsafe impl NSObjectProtocol for JiraSetupFormView {}
    );

    thread_local! {
        static LIVE_FORM: RefCell<Option<objc2::rc::Weak<JiraSetupFormView>>> = const { RefCell::new(None) };
    }

    /// Run `job` off the main thread, then `then` on main with the live form
    /// (skipped if it went away meanwhile).
    fn background<R: Send + 'static>(
        job: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&JiraSetupFormView, R) + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let r = job();
            crate::platform::run_on_main(move || {
                if let Some(f) = LIVE_FORM.with(|l| l.borrow().as_ref().and_then(|w| w.load())) {
                    then(&f, r);
                }
            });
        });
    }

    impl JiraSetupFormView {
        /// Build the embeddable form (one retained view; no window).
        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let settings = SetupStrings::load();
            let text_color = colors.text.to_nscolor();
            let intro = NSTextField::wrappingLabelWithString(&NSString::from_str(&settings.intro), mtm);
            intro.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            intro.setTextColor(Some(&text_color));

            let label = |text: &str| {
                let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
                l.setAlignment(NSTextAlignment::Right);
                l.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                l.setTextColor(Some(&text_color));
                l
            };
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
            let site = field(&settings.site_placeholder);
            let email = field(&settings.email_placeholder);
            let token: Retained<NSTextField> = Retained::into_super(NSSecureTextField::new(mtm));
            token.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            token.setTextColor(Some(&text_color));
            token.setPlaceholderString(Some(&NSString::from_str(&settings.token_placeholder)));
            token.setUsesSingleLineMode(true);
            if let Some(cell) = token.cell() {
                cell.setWraps(false);
                cell.setScrollable(true);
            }
            let project = field(&settings.projects_placeholder);
            let max = field(&settings.max_placeholder);

            let notice = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
            notice.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            notice.setTextColor(Some(&colors.dim.to_nscolor()));
            let result = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
            result.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            result.setTextColor(Some(&colors.dim.to_nscolor()));

            let button = |title: &str| {
                // Themed capsule (was a system `Push` bezel that ignored
                // `[theme]` — the same ban as `NSSegmentedControl`).
                let b = crate::ui::outline_button::OutlineButton::new(mtm, colors, title);
                b.set_tone(crate::ui::outline_button::Tone::Neutral);
                b
            };
            let test = button(&settings.test_label);
            let curl = button(&settings.curl_label);
            curl.setToolTip(Some(&NSString::from_str(&settings.curl_tooltip)));
            let save = button(&settings.save_label);

            let row_labels = [
                label(&settings.site_label),
                label(&settings.email_label),
                label(&settings.token_label),
                label(&settings.projects_label),
                label(&settings.max_label),
            ];

            let this = Self::alloc(mtm).set_ivars(FormIvars {
                settings,
                colors,
                row_labels,
                intro,
                site,
                email,
                token,
                project,
                max,
                notice,
                result,
                buttons: [test, curl, save],
                detected: RefCell::new(None),
                sink: RefCell::new(None),
                busy: Cell::new(false),
            });
            let v: Retained<Self> = unsafe {
                msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, FORM_HEIGHT))]
            };
            v.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            {
                let i = v.ivars();
                let mut subs: Vec<&NSView> = vec![
                    &*i.intro,
                    &*i.site,
                    &*i.email,
                    &*i.token,
                    &*i.project,
                    &*i.max,
                    &*i.notice,
                    &*i.result,
                ];
                for l in &i.row_labels {
                    subs.push(&**l);
                }
                for b in &i.buttons {
                    subs.push(&**b);
                }
                for s in subs {
                    v.addSubview(s);
                }
                let target: &AnyObject = as_any(&*v);
                for (b, s) in [
                    (&i.buttons[0], sel!(testClicked:)),
                    (&i.buttons[1], sel!(curlClicked:)),
                    (&i.buttons[2], sel!(saveClicked:)),
                ] {
                    b.set_target_action(target, s);
                }
            }
            LIVE_FORM.with(|l| *l.borrow_mut() = Some(objc2::rc::Weak::from_retained(&v)));
            v.layout();
            v
        }

        /// The section heading the Config body draws above the form.
        pub fn section_title(&self) -> String {
            self.ivars().settings.section_title.clone()
        }

        /// The fixed form height the Config body reserves.
        pub fn height(&self) -> f64 {
            FORM_HEIGHT
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(SetupOutcome)>) {
            *self.ivars().sink.borrow_mut() = Some(Rc::from(cb));
        }

        fn emit(&self, o: SetupOutcome) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(o);
            }
        }

        /// `prefill()`: fill blank fields from `jira_config.py --check`.
        pub fn prefill(&self) {
            background(
                || run_stdin("jira_config.py", &["--check"], None).1,
                |f, out| {
                    let Ok(d) = serde_json::from_str::<Value>(&out) else { return };
                    let i = f.ivars();
                    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    let set = |fld: &NSTextField, val: &str| {
                        if fld.stringValue().length() == 0 && !val.is_empty() {
                            fld.setStringValue(&NSString::from_str(val));
                        }
                    };
                    set(&i.site, &s("site"));
                    if s("auth") != "bearer" {
                        set(&i.email, &s("email"));
                    }
                    let keys: Vec<&str> = d
                        .get("projectKeys")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let project = if keys.is_empty() { s("defaultProject") } else { keys.join(", ") };
                    set(&i.project, &project);
                    let m = d.get("defaultMax").and_then(Value::as_i64).unwrap_or(i.settings.max_default);
                    set(&i.max, &m.to_string());
                    if d.get("hasToken").and_then(Value::as_bool) == Some(true) {
                        i.token.setPlaceholderString(Some(&NSString::from_str(&i.settings.token_kept)));
                    }
                    i.notice.setStringValue(&NSString::from_str(&prefill_notice(&d, &i.settings)));
                },
            );
        }

        fn values(&self) -> SetupValues {
            let t = |f: &NSTextField| f.stringValue().to_string().trim().to_string();
            let i = self.ivars();
            SetupValues { site: t(&i.site), email: t(&i.email), token: t(&i.token), project: t(&i.project), max: t(&i.max) }
        }

        fn set_result(&self, text: &str, ok: Option<bool>) {
            let r = &self.ivars().result;
            r.setStringValue(&NSString::from_str(text));
            let c = match ok {
                Some(true) => self.ivars().colors.tone(PopupTone::Success),
                Some(false) => self.ivars().colors.tone(PopupTone::Danger),
                None => self.ivars().colors.dim,
            };
            r.setTextColor(Some(&c.to_nscolor()));
        }

        fn busy(&self, on: bool) {
            for b in &self.ivars().buttons {
                b.set_enabled(!on);
            }
        }

        fn focus_project(&self) {
            if let Some(w) = self.window() {
                w.makeFirstResponder(Some(&self.ivars().project));
            }
        }

        fn test(&self) {
            self.busy(true);
            self.set_result(&self.ivars().settings.testing, None);
            let v = self.values();
            let hint = self.ivars().settings.detect_hint.clone();
            background(
                move || detect(&v, &hint),
                |f, r| {
                    f.busy(false);
                    match r {
                        Ok((auth, user)) => {
                            *f.ivars().detected.borrow_mut() = Some(auth.clone());
                            let auth = auth_title(&f.ivars().settings, &auth).to_string();
                            let msg = fill(&f.ivars().settings.connected, &[("user", &user), ("auth", &auth)]);
                            f.set_result(&msg, Some(true));
                        }
                        Err(msg) => {
                            let m = fill(&f.ivars().settings.test_failed, &[("error", &msg)]);
                            f.set_result(&m, Some(false));
                        }
                    }
                },
            );
        }

        fn copy_curl(&self) {
            let v = self.values();
            let auth = self.ivars().detected.borrow().clone().unwrap_or_else(|| {
                if v.email.is_empty() { "bearer".into() } else { "basic".into() }
            });
            background(
                move || {
                    let (args, stdin) = v.typed_args();
                    let mut all = vec!["--curl".to_string(), "--myself".into(), "--auth".into(), auth];
                    all.extend(args);
                    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
                    run_stdin("jira_api.py", &refs, stdin.as_deref())
                },
                |f, (code, out, err)| {
                    let cmd = out.trim();
                    if code != 0 || cmd.is_empty() {
                        let e = error_line(&err, &format!("could not build curl (exit {code})"));
                        let m = fill(&f.ivars().settings.curl_failed, &[("error", &e)]);
                        f.set_result(&m, Some(false));
                        return;
                    }
                    let _ = crate::platform::set_text(cmd);
                    f.set_result(&f.ivars().settings.curl_copied, None);
                },
            );
        }

        /// `saveAndEnable(_:)` — save, set the projects in scope, enable.
        fn save(&self) {
            let v = self.values();
            if v.site.is_empty() || !v.site.starts_with("http") {
                self.set_result(&self.ivars().settings.site_required, Some(false));
                return;
            }
            self.busy(true);
            self.set_result(&self.ivars().settings.detecting, None);
            let detected = self.ivars().detected.borrow().clone();
            let hint = self.ivars().settings.detect_hint.clone();
            let max_default = self.ivars().settings.max_default;
            background(
                move || {
                    let (keys, bad) = parse_project_keys(&v.project);
                    if !bad.is_empty() {
                        return Err(SaveErr::BadKeys(bad.join(", ")));
                    }
                    if keys.is_empty() {
                        return Err(SaveErr::NoProjects);
                    }
                    let det = detect(&v, &hint);
                    let mode = match &det {
                        Ok((a, _)) => a.clone(),
                        Err(_) => detected.unwrap_or_else(|| if v.email.is_empty() { "bearer".into() } else { "basic".into() }),
                    };
                    let obj = v.save_object(&mode, &keys, max_default).to_string();
                    let (code, _, err) = run_stdin("jira_config.py", &["--save"], Some(&obj));
                    if code != 0 {
                        return Err(SaveErr::Save(error_line(&err, &format!("exit {code}"))));
                    }
                    let (code, out, err) =
                        run_stdin("jira_config.py", &["--team-set", "project_keys"], Some(&json!(keys).to_string()));
                    let r: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
                    if code != 0 || r.get("ok").and_then(Value::as_bool) != Some(true) {
                        let probs: Vec<&str> = r
                            .get("problems")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        let why = if probs.is_empty() { error_line(&err, &format!("exit {code}")) } else { probs.join("; ") };
                        return Err(SaveErr::Projects(why));
                    }
                    Ok((det, mode))
                },
                |f, r| {
                    f.busy(false);
                    let st = &f.ivars().settings;
                    match r {
                        Err(SaveErr::BadKeys(keys)) => {
                            let m = fill(&st.bad_keys, &[("keys", &keys)]);
                            f.set_result(&m, Some(false));
                            f.focus_project();
                        }
                        Err(SaveErr::NoProjects) => {
                            f.set_result(&st.need_project, Some(false));
                            f.focus_project();
                        }
                        Err(SaveErr::Save(e)) => {
                            let m = fill(&st.save_failed, &[("error", &e)]);
                            f.set_result(&m, Some(false));
                        }
                        Err(SaveErr::Projects(e)) => {
                            let m = fill(&st.projects_failed, &[("error", &e)]);
                            f.set_result(&m, Some(false));
                        }
                        Ok((Err(msg), _)) => {
                            let m = fill(&st.login_failed, &[("error", &msg)]);
                            f.set_result(&m, Some(false));
                        }
                        Ok((Ok((_, user)), mode)) => {
                            let auth = auth_title(st, &mode).to_string();
                            let m = fill(&st.enabling, &[("user", &user), ("auth", &auth)]);
                            f.set_result(&m, Some(true));
                            f.emit(SetupOutcome::Saved);
                        }
                    }
                },
            );
        }

        /// Lay the widgets out for the current width (top-left origin).
        fn layout(&self) {
            let i = self.ivars();
            let w = self.bounds().size.width.max(320.0);
            let field_x = INNER + LABEL_W + 8.0;
            let field_w = (w - field_x - INNER).max(80.0);
            let mut y = 0.0;
            i.intro.setFrame(rect(INNER, y, w - INNER * 2.0, INTRO_H));
            y += INTRO_H + 6.0;
            let fields = [&i.site, &i.email, &i.token, &i.project, &i.max];
            for (n, l) in i.row_labels.iter().enumerate() {
                l.setFrame(rect(INNER, y + 3.0, LABEL_W, 18.0));
                fields[n].setFrame(rect(field_x, y, field_w, FIELD_H));
                y += ROW_H;
            }
            i.notice.setFrame(rect(INNER, y, w - INNER * 2.0, NOTICE_H));
            y += NOTICE_H + 4.0;
            i.result.setFrame(rect(INNER, y, w - INNER * 2.0, RESULT_H));
            y += RESULT_H + 6.0;
            i.buttons[0].setFrame(rect(INNER, y, 140.0, BUTTON_H));
            i.buttons[1].setFrame(rect(INNER + 148.0, y, 100.0, BUTTON_H));
            i.buttons[2].setFrame(rect(w - INNER - 130.0, y, 130.0, BUTTON_H));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_args_and_save_object() {
        let v = SetupValues {
            site: "https://j.example".into(),
            email: "a@b".into(),
            token: "tok".into(),
            project: "KAN".into(),
            max: "".into(),
        };
        let (args, stdin) = v.typed_args();
        assert_eq!(args, ["--email", "a@b", "--site", "https://j.example", "--token-stdin"]);
        assert_eq!(stdin.as_deref(), Some("tok\n"));
        let o = v.save_object("bearer", &["KAN".into(), "SAM1".into()], 25);
        assert_eq!(o["email"], "");
        assert_eq!(o["defaultProject"], "KAN");
        assert_eq!(o["defaultMax"], 25);
        assert_eq!(o["token"], "tok");
        assert_eq!(v.save_object("basic", &[], 25)["email"], "a@b");
    }

    #[test]
    fn read_detect_reports_tries() {
        let hint = SetupStrings::default().detect_hint;
        assert_eq!(read_detect(0, r#"{"auth":"bearer","user":"Ann"}"#, "", &hint), Ok(("bearer".into(), "Ann".into())));
        let e = read_detect(1, r#"{"error":"401","tried":[{"mode":"bearer","http":401}]}"#, "", &hint).unwrap_err();
        assert_eq!(e, "401 (tried bearer → HTTP 401) — Copy curl to reproduce in a terminal");
        assert!(read_detect(2, "", "boom\n", &hint).unwrap_err().starts_with("boom"));
    }

    #[test]
    fn setup_strings_read_from_section_with_defaults() {
        let d = SetupStrings::default();
        assert_eq!(SetupStrings::from_section(&HashMap::new()), d);

        let mut m = HashMap::new();
        m.insert("setup-section-title".to_string(), "Connection".to_string());
        m.insert("setup-save-label".to_string(), "Save!".to_string());
        m.insert("setup-max-default".to_string(), "50".to_string());
        let s = SetupStrings::from_section(&m);
        assert_eq!(s.section_title, "Connection");
        assert_eq!(s.save_label, "Save!");
        assert_eq!(s.max_default, 50);
        // Untouched keys keep their defaults.
        assert_eq!(s.test_label, d.test_label);
        assert_eq!(s.auth_bearer, d.auth_bearer);
    }

    #[test]
    fn fill_replaces_named_markers() {
        let s = SetupStrings::default();
        let out = fill(&s.connected, &[("user", "Ann"), ("auth", &s.auth_bearer)]);
        assert_eq!(out, "✓ Connected as Ann — Bearer token (Server / Data Center)");
    }

    #[test]
    fn prefill_notice_flags_env_overrides() {
        let s = SetupStrings::default();
        let d: serde_json::Value = serde_json::from_str(r#"{"notes":["using env JIRA_SITE"]}"#).unwrap();
        let n = prefill_notice(&d, &s);
        assert!(n.starts_with("Saves to "));
        assert!(n.contains("Using JIRA_SITE from the environment"));
    }
}
