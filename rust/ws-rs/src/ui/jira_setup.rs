//! `JiraSetupWindow` (kitchen_sink.swift): the Site / Email / Token /
//! Projects-in-scope / Max-results form. Test Connection detects the auth type
//! (`jira_api.py --detect-auth`), Copy curl builds the login test, and Save &
//! Enable writes `jira_config.py --save` + `--team-set project_keys`, then
//! turns polling on. The Python calls run off the main thread; their answers
//! hop back with `DispatchQueue::main()`.

use serde_json::{json, Value};

use crate::app::jira_poll::{error_line, run_stdin};

/// `JiraSetupWindow.authTitle(_:)`.
pub fn auth_title(mode: &str) -> &'static str {
    if mode == "basic" {
        "email + API token (Cloud)"
    } else {
        "Bearer token (Server / Data Center)"
    }
}

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
    pub fn save_object(&self, mode: &str, keys: &[String]) -> Value {
        let mut o = json!({
            "site": self.site,
            "email": if mode == "basic" { self.email.as_str() } else { "" },
            "auth": mode,
            "defaultProject": keys.first().cloned().unwrap_or_default(),
            "defaultMax": self.max.parse::<i64>().unwrap_or(25),
        });
        if !self.token.is_empty() {
            o["token"] = json!(self.token);
        }
        o
    }
}

/// `--detect-auth`'s answer: `Ok((auth, user))` or the failure line.
pub fn read_detect(code: i32, out: &str, err: &str) -> Result<(String, String), String> {
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
    Err(format!("{msg}{tried} — Copy curl to reproduce in a terminal"))
}

/// `detect(done:)`, blocking.
fn detect(v: &SetupValues) -> Result<(String, String), String> {
    let (args, stdin) = v.typed_args();
    let mut all = vec!["--detect-auth".to_string()];
    all.extend(args);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let (code, out, err) = run_stdin("jira_api.py", &refs, stdin.as_deref());
    read_detect(code, &out, &err)
}

/// `parseProjectKeys(_:)` through the helper (`jira.project_keys`).
fn parse_project_keys(raw: &str) -> (Vec<String>, Vec<String>) {
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
        Err(_) => (Vec::new(), Vec::new()),
    }
}

/// `JiraPoll.configPath`.
pub fn config_path() -> String {
    std::env::var("JIRA_CONFIG_JSON").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| {
        format!("{}/.config/jira/config.json", std::env::var("HOME").unwrap_or_default())
    })
}

/// `prefill()`'s notice lines from the `--check` JSON.
pub fn prefill_notice(d: &Value) -> String {
    let notes: Vec<&str> = d.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut lines = vec![format!("Saves to {} (chmod 600).", config_path())];
    for k in ["SITE", "EMAIL", "TOKEN"] {
        if notes.iter().any(|n| n.contains(&format!("env JIRA_{k}"))) {
            lines.push(format!("Using JIRA_{k} from the environment — save will persist it into config.json."));
        }
    }
    lines.join("\n")
}

#[cfg(target_os = "macos")]
pub use appkit::show;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSEvent, NSEventMask, NSFont,
        NSSecureTextField, NSText, NSTextAlignment, NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
    };
    use objc2_foundation::{NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;

    const W: f64 = 520.0;
    const H: f64 = 400.0;

    pub struct Ivars {
        window: Retained<NSWindow>,
        site: Retained<NSTextField>,
        email: Retained<NSTextField>,
        token: Retained<NSTextField>,
        project: Retained<NSTextField>,
        max: Retained<NSTextField>,
        notice: Retained<NSTextField>,
        result: Retained<NSTextField>,
        buttons: Vec<Retained<NSButton>>,
        detected: RefCell<Option<String>>,
        monitor: RefCell<Option<Retained<AnyObject>>>,
        on_enabled: RefCell<Option<Box<dyn Fn()>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraSetupWindow"]
        #[ivars = Ivars]
        pub struct JiraSetup;

        impl JiraSetup {
            #[unsafe(method(test:))]
            fn test_action(&self, _s: Option<&AnyObject>) {
                self.test();
            }

            #[unsafe(method(save:))]
            fn save_action(&self, _s: Option<&AnyObject>) {
                self.save();
            }

            #[unsafe(method(curl:))]
            fn curl_action(&self, _s: Option<&AnyObject>) {
                self.copy_curl();
            }

            #[unsafe(method(cancel:))]
            fn cancel_action(&self, _s: Option<&AnyObject>) {
                self.close();
            }
        }

        unsafe impl NSObjectProtocol for JiraSetup {}

        unsafe impl NSWindowDelegate for JiraSetup {
            #[unsafe(method(windowWillClose:))]
            fn window_will_close(&self, _n: &NSNotification) {
                self.drop_monitor();
                LIVE.with(|l| l.borrow_mut().take());
            }
        }
    );

    thread_local! {
        static LIVE: RefCell<Option<Retained<JiraSetup>>> = const { RefCell::new(None) };
    }

    /// Run `job` off the main thread, then `then` on main with the live
    /// window (skipped if it closed meanwhile).
    fn background<R: Send + 'static>(job: impl FnOnce() -> R + Send + 'static, then: impl FnOnce(&JiraSetup, R) + Send + 'static) {
        std::thread::spawn(move || {
            let r = job();
            dispatch2::DispatchQueue::main().exec_async(move || {
                if let Some(w) = LIVE.with(|l| l.borrow().clone()) {
                    then(&w, r);
                }
            });
        });
    }

    /// `JiraSetupWindow.show(controller:reason:)`. `on_enabled` runs after a
    /// successful Save & Enable (the host's `setJiraEnabled(true)`).
    pub fn show(reason: Option<&str>, on_enabled: Box<dyn Fn()>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(w) = LIVE.with(|l| l.borrow().clone()) {
            if let Some(r) = reason {
                w.set_result(r, None);
            }
            *w.ivars().on_enabled.borrow_mut() = Some(on_enabled);
            NSApplication::sharedApplication(mtm).activate();
            w.ivars().window.makeKeyAndOrderFront(None);
            return;
        }
        let w = JiraSetup::build(mtm, on_enabled);
        LIVE.with(|l| *l.borrow_mut() = Some(w.clone()));
        if let Some(r) = reason {
            w.set_result(r, None);
        }
        w.prefill();
        NSApplication::sharedApplication(mtm).activate();
        let win = &w.ivars().window;
        win.center();
        win.makeKeyAndOrderFront(None);
        win.makeFirstResponder(Some(&w.ivars().site));
    }

    fn label(mtm: MainThreadMarker, text: &str, wrapping: bool) -> Retained<NSTextField> {
        let s = NSString::from_str(text);
        if wrapping {
            NSTextField::wrappingLabelWithString(&s, mtm)
        } else {
            NSTextField::labelWithString(&s, mtm)
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    impl JiraSetup {
        fn build(mtm: MainThreadMarker, on_enabled: Box<dyn Fn()>) -> Retained<Self> {
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect(0.0, 0.0, W, H),
                    NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            window.setTitle(&NSString::from_str("Jira Setup"));
            unsafe { window.setReleasedWhenClosed(false) };
            window.setLevel(objc2_app_kit::NSPopUpMenuWindowLevel + 1);
            let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, W, H));
            let head = label(
                mtm,
                "Paste your token — the auth type is detected on Test / Save. Server / Data Center personal access tokens go out as Authorization: Bearer; a Jira Cloud API token also needs the account email.",
                true,
            );
            head.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            head.setFrame(rect(20.0, H - 80.0, W - 40.0, 64.0));
            content.addSubview(&head);
            let field = || NSTextField::new(mtm);
            let site = field();
            let email = field();
            let token: Retained<NSTextField> = Retained::into_super(NSSecureTextField::new(mtm));
            let project = field();
            let max = field();
            let mut y = H - 116.0;
            for (text, f, placeholder) in [
                ("Site URL", &site, "https://jira.example.com"),
                ("Email (Cloud only)", &email, "only for *.atlassian.net — blank for a personal access token"),
                ("Token", &token, "personal access token / API token"),
                ("Projects in scope", &project, "e.g. SAM1, KAN — required; every query stays inside these"),
                ("Max results", &max, "25"),
            ] {
                let l = label(mtm, text, false);
                l.setAlignment(NSTextAlignment::Right);
                l.setFrame(rect(20.0, y + 3.0, 110.0, 18.0));
                f.setFrame(rect(140.0, y, W - 160.0, 24.0));
                f.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                f.setUsesSingleLineMode(true);
                if let Some(cell) = f.cell() {
                    cell.setWraps(false);
                    cell.setScrollable(true);
                }
                content.addSubview(&l);
                content.addSubview(f);
                y -= 34.0;
            }
            let notice = label(mtm, "", true);
            notice.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            notice.setTextColor(Some(&NSColor::secondaryLabelColor()));
            notice.setFrame(rect(20.0, y - 18.0, W - 40.0, 40.0));
            content.addSubview(&notice);
            let result = label(mtm, "", true);
            result.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            result.setFrame(rect(20.0, 58.0, W - 40.0, 44.0));
            content.addSubview(&result);
            let button = |title: &str| {
                let b = NSButton::new(mtm);
                b.setTitle(&NSString::from_str(title));
                b.setBezelStyle(NSBezelStyle::Push);
                b
            };
            let test = button("Test Connection");
            let curl = button("Copy curl");
            let cancel = button("Cancel");
            let save = button("Save & Enable");
            curl.setToolTip(Some(&NSString::from_str("Copy the login test (GET /rest/api/2/myself) as a curl command")));
            cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
            save.setKeyEquivalent(&NSString::from_str("\r"));
            save.setFrame(rect(W - 20.0 - 130.0, 16.0, 130.0, 30.0));
            cancel.setFrame(rect(W - 20.0 - 130.0 - 96.0, 16.0, 90.0, 30.0));
            test.setFrame(rect(20.0, 16.0, 140.0, 30.0));
            curl.setFrame(rect(166.0, 16.0, 100.0, 30.0));
            for b in [&test, &curl, &cancel, &save] {
                content.addSubview(b);
            }
            window.setContentView(Some(&content));
            window.setAutorecalculatesKeyViewLoop(true);
            window.setInitialFirstResponder(Some(&site));
            let this = Self::alloc(mtm).set_ivars(Ivars {
                window: window.clone(),
                site,
                email,
                token,
                project,
                max,
                notice,
                result,
                buttons: vec![test.clone(), curl.clone(), save.clone()],
                detected: RefCell::new(None),
                monitor: RefCell::new(None),
                on_enabled: RefCell::new(Some(on_enabled)),
            });
            let this: Retained<Self> = unsafe { msg_send![super(this), init] };
            let target: &AnyObject = unsafe { &*(&*this as *const Self as *const AnyObject) };
            for (b, s) in [(&test, sel!(test:)), (&curl, sel!(curl:)), (&cancel, sel!(cancel:)), (&save, sel!(save:))] {
                unsafe {
                    b.setTarget(Some(target));
                    b.setAction(Some(s));
                }
            }
            window.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*this)));
            this.install_edit_shortcuts();
            this
        }

        /// `installEditShortcuts()`: Esc closes; Cmd/Ctrl+V/C, Cmd+A/X/Z go to
        /// the field editor (rule 1).
        fn install_edit_shortcuts(&self) {
            let window = self.ivars().window.clone();
            let block = block2::RcBlock::new(move |e: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
                let ev = unsafe { e.as_ref() };
                if !window.isKeyWindow() {
                    return e.as_ptr();
                }
                if ev.keyCode() == 53 {
                    if let Some(w) = LIVE.with(|l| l.borrow().clone()) {
                        w.close();
                    }
                    return std::ptr::null_mut();
                }
                let m = ev.modifierFlags();
                let cmd = m.contains(objc2_app_kit::NSEventModifierFlags::Command);
                let ctrl = m.contains(objc2_app_kit::NSEventModifierFlags::Control);
                if !(cmd || ctrl) {
                    return e.as_ptr();
                }
                let Some(fr) = window.firstResponder() else { return e.as_ptr() };
                let Some(ed) = (&*fr as &AnyObject).downcast_ref::<NSText>() else { return e.as_ptr() };
                // SAFETY: standard responder actions with a nil sender.
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

        fn values(&self) -> SetupValues {
            let t = |f: &NSTextField| f.stringValue().to_string().trim().to_string();
            let i = self.ivars();
            SetupValues { site: t(&i.site), email: t(&i.email), token: t(&i.token), project: t(&i.project), max: t(&i.max) }
        }

        fn set_result(&self, text: &str, ok: Option<bool>) {
            let r = &self.ivars().result;
            r.setStringValue(&NSString::from_str(text));
            let c = match ok {
                Some(true) => NSColor::systemGreenColor(),
                Some(false) => NSColor::systemRedColor(),
                None => NSColor::secondaryLabelColor(),
            };
            r.setTextColor(Some(&c));
        }

        fn busy(&self, on: bool) {
            for b in &self.ivars().buttons {
                b.setEnabled(!on);
            }
        }

        /// `prefill()`: fill blank fields from `jira_config.py --check`.
        fn prefill(&self) {
            background(
                || run_stdin("jira_config.py", &["--check"], None).1,
                |w, out| {
                    let Ok(d) = serde_json::from_str::<Value>(&out) else { return };
                    let i = w.ivars();
                    let empty = |f: &NSTextField| f.stringValue().length() == 0;
                    let set = |f: &NSTextField, v: &str| f.setStringValue(&NSString::from_str(v));
                    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    if empty(&i.site) {
                        set(&i.site, &s("site"));
                    }
                    if empty(&i.email) && s("auth") != "bearer" {
                        set(&i.email, &s("email"));
                    }
                    if empty(&i.project) {
                        let keys: Vec<&str> = d
                            .get("projectKeys")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        set(&i.project, &if keys.is_empty() { s("defaultProject") } else { keys.join(", ") });
                    }
                    if empty(&i.max) {
                        if let Some(m) = d.get("defaultMax").and_then(Value::as_i64) {
                            set(&i.max, &m.to_string());
                        }
                    }
                    if d.get("hasToken").and_then(Value::as_bool) == Some(true) {
                        i.token.setPlaceholderString(Some(&NSString::from_str("token set — leave blank to keep it")));
                    }
                    set(&i.notice, &prefill_notice(&d));
                },
            );
        }

        fn test(&self) {
            self.busy(true);
            self.set_result("Testing (detecting the auth type)…", None);
            let v = self.values();
            background(
                move || detect(&v),
                |w, r| {
                    w.busy(false);
                    match r {
                        Ok((auth, user)) => {
                            *w.ivars().detected.borrow_mut() = Some(auth.clone());
                            w.set_result(&format!("✓ Connected as {user} — {}", auth_title(&auth)), Some(true));
                        }
                        Err(msg) => w.set_result(&format!("✗ {msg}"), Some(false)),
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
                |w, (code, out, err)| {
                    let cmd = out.trim();
                    if code != 0 || cmd.is_empty() {
                        w.set_result(&format!("✗ {}", error_line(&err, &format!("could not build curl (exit {code})"))), Some(false));
                        return;
                    }
                    let pb = objc2_app_kit::NSPasteboard::generalPasteboard();
                    pb.clearContents();
                    let _ = unsafe { pb.setString_forType(&NSString::from_str(cmd), objc2_app_kit::NSPasteboardTypeString) };
                    w.set_result("curl copied (includes the token) — paste into a terminal to test the login.", None);
                },
            );
        }

        /// `saveAndEnable(_:)`.
        fn save(&self) {
            let v = self.values();
            if v.site.is_empty() || !v.site.starts_with("http") {
                self.set_result("✗ Site URL must start with https://", Some(false));
                return;
            }
            self.busy(true);
            self.set_result("Detecting the auth type…", None);
            let detected = self.ivars().detected.borrow().clone();
            background(
                move || {
                    let (keys, bad) = parse_project_keys(&v.project);
                    if !bad.is_empty() {
                        return Err((format!("✗ Not a project key: {} — use keys like SAM1, KAN", bad.join(", ")), true));
                    }
                    if keys.is_empty() {
                        return Err(("✗ Enter at least one project in scope — every query is limited to these projects".into(), true));
                    }
                    let det = detect(&v);
                    let mode = match &det {
                        Ok((a, _)) => a.clone(),
                        Err(_) => detected.unwrap_or_else(|| if v.email.is_empty() { "bearer".into() } else { "basic".into() }),
                    };
                    let obj = v.save_object(&mode, &keys).to_string();
                    let (code, _, err) = run_stdin("jira_config.py", &["--save"], Some(&obj));
                    if code != 0 {
                        return Err((format!("✗ save failed: {}", error_line(&err, &format!("exit {code}"))), false));
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
                        return Err((format!("✗ projects in scope not saved: {why}"), false));
                    }
                    Ok((det, mode))
                },
                |w, r| {
                    w.busy(false);
                    match r {
                        Err((msg, focus_project)) => {
                            w.set_result(&msg, Some(false));
                            if focus_project {
                                w.ivars().window.makeFirstResponder(Some(&w.ivars().project));
                            }
                        }
                        Ok((Err(msg), _)) => {
                            w.set_result(&format!("✗ saved, but login failed: {msg} — polling stays off"), Some(false));
                        }
                        Ok((Ok((_, user)), mode)) => {
                            w.set_result(&format!("✓ Connected as {user} — {} — enabling…", auth_title(&mode)), Some(true));
                            let cb = w.ivars().on_enabled.borrow_mut().take();
                            w.close();
                            if let Some(cb) = cb {
                                cb();
                            }
                        }
                    }
                },
            );
        }

        fn close(&self) {
            self.ivars().window.orderOut(None);
            self.drop_monitor();
            LIVE.with(|l| l.borrow_mut().take());
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
        let o = v.save_object("bearer", &["KAN".into(), "SAM1".into()]);
        assert_eq!(o["email"], "");
        assert_eq!(o["defaultProject"], "KAN");
        assert_eq!(o["defaultMax"], 25);
        assert_eq!(o["token"], "tok");
        assert_eq!(v.save_object("basic", &[])["email"], "a@b");
    }

    #[test]
    fn read_detect_reports_tries() {
        assert_eq!(read_detect(0, r#"{"auth":"bearer","user":"Ann"}"#, ""), Ok(("bearer".into(), "Ann".into())));
        let e = read_detect(1, r#"{"error":"401","tried":[{"mode":"bearer","http":401}]}"#, "").unwrap_err();
        assert_eq!(e, "401 (tried bearer → HTTP 401) — Copy curl to reproduce in a terminal");
        assert!(read_detect(2, "", "boom\n").unwrap_err().starts_with("boom"));
    }
}
