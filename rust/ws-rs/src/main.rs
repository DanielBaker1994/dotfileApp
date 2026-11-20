//! kitchen-sink — the app's entry point (the former Swift `main.swift`).
//!
//! One-shot commands, the install check, the CLI client pass (`app::cli`),
//! then the daemon. The module tree mirrors the old Swift files.

#![allow(dead_code)]

mod app;
mod engines;
mod panes;
mod platform;
mod ui;
mod views;

// The egui frontend: always on off macOS; on macOS only when built with
// `--features egui` (the `WS_FRONTEND=egui` dev switch).
#[cfg(any(not(target_os = "macos"), feature = "egui"))]
mod frontend_egui;

use std::sync::Arc;
use std::sync::Mutex;

use app::host::SwitcherController;
use app::paths::Paths;
use app::python_helper::PythonHelper;
use app::socket::{CommandHandler, CommandServer, DaemonLock};

fn main() {
    std::env::set_var("PYTHONDONTWRITEBYTECODE", "1");

    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).cloned();
    // `headless` / `--headless`, or `WS_HEADLESS=1` when no argv mode is given:
    // run the daemon without AppKit — socket API only (no window, menu, status
    // item or Dock presence; `do:` dispatch runs inline on the socket thread).
    let headless = match mode.as_deref() {
        Some("headless" | "--headless") => true,
        Some(_) => false,
        None => std::env::var("WS_HEADLESS").map(|v| v == "1").unwrap_or(false),
    };

    // Non-daemon one-shot commands (the ws-settings / CLI fast path).
    if let Some(cmd) = mode.as_deref() {
        match cmd {
            "help" | "--help" | "-h" => {
                print_usage();
                return;
            }
            "config-schema" => {
                println!("{}", config_schema());
                return;
            }
            "config-check" => {
                std::process::exit(config_check(&args[2..]));
            }
            _ => {}
        }
    }

    configure_helper();
    let paths = Paths::from_env();
    ensure_home(&paths);

    // The CLI client pass (`main.swift`): forward to a running daemon, print
    // replies, or come back holding the daemon lock.
    match app::cli::run(&args, &paths) {
        app::cli::Outcome::Exit(code) => std::process::exit(code),
        app::cli::Outcome::Daemon { mode, lock } => {
            let _lock: DaemonLock = lock;
            run_daemon(paths.notes_socket_path(), mode, headless);
        }
    }
}

/// `AppInstall.ensureHome()`: an installed app (not a repo build, not running
/// from the disk image) keeps `~/.config/kitchen-sink` set up for its version.
fn ensure_home(paths: &Paths) {
    let version = crate::platform::bundle_version();
    views::setup::AppInstall::from_paths(paths, version).ensure_home(false);
}

/// Point the Python helper at the right `pylib` and export the config path
/// for a non-repo build (mirrors `main.swift`'s top-level setup).
fn configure_helper() {
    let paths = Paths::from_env();
    if !paths.is_repo_build() {
        std::env::set_var("WS_COMMANDS_CONF", paths.commands_conf_path());
    }
    // Dev override: a non-dist bundle has no Resources/pylib, so allow pointing
    // the helper at a checkout's pylib (the parity harness sets this).
    let lib_dir = match std::env::var("WS_RS_ASSET_DIR") {
        Ok(dir) if !dir.is_empty() => format!("{dir}/pylib"),
        _ => format!("{}/pylib", paths.asset_dir()),
    };
    PythonHelper::shared().configure(&lib_dir);
}

fn run_daemon(sock: String, mode: Option<String>, headless: bool) {
    let mut controller = SwitcherController::new_default();
    controller.reload_config();
    controller.register_views();
    let controller = Arc::new(controller);

    let _server = match CommandServer::start(&sock, controller.clone()) {
        Ok(s) => s,
        Err(e) => {
            app::logging::ws_log(&format!("cannot bind command socket {sock}: {e}"));
            std::process::exit(1);
        }
    };

    if headless {
        // No AppKit at all: no NSApplication, menu, status item, UI queues or
        // launch presentation. `state`/`do:`/raw verbs run inline on the
        // socket thread; parking keeps `_server` (and the caller's `_lock`)
        // alive so the process stays the single daemon instance.
        app::logging::ws_log(&format!("kitchen-sink (rust) headless daemon: socket {sock}"));
        loop {
            std::thread::park();
        }
    }

    // The egui frontend is the UI on non-macOS targets (Wave 4+).
    #[cfg(not(target_os = "macos"))]
    {
        run_egui_frontend(controller, mode.as_deref(), &sock);
    }

    // macOS dev switch: `--features egui` + `WS_FRONTEND=egui` runs the egui
    // frontend instead of AppKit (for iterating on the cross-platform UI). The
    // AppKit path below is otherwise untouched.
    #[cfg(all(target_os = "macos", feature = "egui"))]
    {
        if std::env::var("WS_FRONTEND").map(|v| v == "egui").unwrap_or(false) {
            run_egui_frontend(controller.clone(), mode.as_deref(), &sock);
            return;
        }
    }

    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let mtm = MainThreadMarker::new().expect("main thread");
        let app = NSApplication::sharedApplication(mtm);
        // Menu-bar-only: no Dock icon or Cmd+Tab entry; the status item (and
        // the hotkeys / `ws` CLI) open the window. `[app] dock-icon = true`
        // brings the Dock icon back.
        let dock = std::env::var("WS_DOCK_ICON").map_or(false, |v| v == "1")
            || app::config::read_config_text()
                .map(|t| dock_icon_enabled(&t))
                .unwrap_or(false);
        app.setActivationPolicy(if dock {
            NSApplicationActivationPolicy::Regular
        } else {
            NSApplicationActivationPolicy::Accessory
        });
        // The main menu's File ▸ "Reset Window Size" row routes through the
        // same click handler as the status item; File ▸ "Close Window" (⌘W)
        // stays on the responder chain. The target is leaked on purpose:
        // `NSMenuItem.target` is weak, so it must outlive the menu (which lives
        // for the whole process).
        {
            use app::menu::{install_main_menu, MenuTarget};
            let c = controller.clone();
            let target = MenuTarget::new(mtm, move |action| status_menu_action(&c, action));
            install_main_menu(mtm, Some(&target));
            std::mem::forget(target);
        }
        install_status_item(mtm, &controller);

        // The socket thread enqueues UI commands; the main-thread timer drains
        // them into the shared host window (see `app::host::install_daemon_ui`).
        let queue: app::host::UiQueue = Arc::new(Mutex::new(Vec::new()));
        controller.install_ui(queue.clone());
        let main_queue: app::host::MainQueue = Arc::new(Mutex::new(Vec::new()));
        controller.install_main_queue(main_queue.clone());
        let _ui = app::host::install_daemon_ui(mtm, controller.clone(), queue, main_queue);

        // A hotkey/CLI invocation that cold-started the daemon applies its mode
        // now; a bare start presents the default view so the app is launchable.
        // (`show` presents the palette only — the shared window stays hidden,
        // mirroring Swift's `showOnLaunch`.)
        if let Some(m) = mode {
            controller.launch_with_prep(&m, None);
        } else {
            controller.open(app::host::DEFAULT_VIEW);
        }

        app::logging::ws_log(&format!("kitchen-sink (rust) daemon: socket {sock}"));
        // Check the config paths at launch: log what is read, and warn on
        // screen if an old ~/.config/jira or ~/.config/confluence is back.
        let (lines, warn) = app::config::startup_config_report();
        for l in &lines {
            app::logging::ws_log(l);
        }
        if let Some(w) = warn {
            crate::ui::toast::show(&w, "exclamationmark.triangle");
        }
        app.run();
    }
}

/// Install the command queues on the controller (exactly like the AppKit
/// `daemon_ui` path does) and hand off to the egui frontend's event loop.
/// This never returns until the egui window closes.
#[cfg(any(not(target_os = "macos"), feature = "egui"))]
fn run_egui_frontend(controller: Arc<SwitcherController>, mode: Option<&str>, sock: &str) {
    let queue: app::host::UiQueue = Arc::new(Mutex::new(Vec::new()));
    controller.install_ui(queue.clone());
    let main_queue: app::host::MainQueue = Arc::new(Mutex::new(Vec::new()));
    controller.install_main_queue(main_queue.clone());

    // A hotkey/CLI invocation that cold-started the daemon applies its mode
    // now; a bare start presents the default view (mirrors the AppKit path).
    if let Some(m) = mode {
        controller.launch_with_prep(m, None);
    } else {
        controller.open(app::host::DEFAULT_VIEW);
    }

    app::logging::ws_log(&format!("kitchen-sink (rust) daemon (egui): socket {sock}"));
    frontend_egui::run(controller, queue, main_queue);
}

/// Install the menu-bar status item from the ported menu spec. The handle is
/// leaked on purpose: the status item lives as long as the process.
#[cfg(target_os = "macos")]
fn install_status_item(mtm: objc2::MainThreadMarker, controller: &Arc<SwitcherController>) {
    use app::menu::{build_menu_spec, install_status_item, MenuSettings};
    let settings = controller.settings();
    let menu_settings = MenuSettings {
        header_style: crate::ui::theme::HeaderStyle::from_raw(settings.header_style.raw())
            .unwrap_or_default(),
        config_path: app::config::tilde(&app::config::commands_conf_path()),
        override_path: app::config::commands_override_path()
            .map(|p| app::config::tilde(&p))
            .unwrap_or_default(),
    };
    let spec = build_menu_spec(&menu_settings);
    let c = controller.clone();
    let handle = install_status_item(mtm, &spec, move |action| status_menu_action(&c, action));
    std::mem::forget(handle);
}

/// Route the menu clicks (status item and main menu) onto the controller.
fn status_menu_action(controller: &Arc<SwitcherController>, action: app::menu::MenuAction) {
    use app::menu::MenuAction;
    match action {
        MenuAction::CloseWindow => controller.hide("menu"),
        MenuAction::Open => controller.open(app::host::DEFAULT_VIEW),
        MenuAction::OpenConfig => {
            platform::open_path(&app::config::commands_conf_path());
        }
        MenuAction::OpenOverrides => {
            if let Some(p) = app::config::ensure_override_file() {
                platform::open_path(&p);
            }
        }
        MenuAction::ResetWindowSize => {
            let _ = controller.do_action("reset-size");
        }
        MenuAction::Settings => {
            let assets = app::paths::Paths::from_env().asset_dir().to_string();
            let bin = format!("{assets}/bin/ws-settings");
            if let Err(e) = std::process::Command::new(&bin).arg("open").spawn() {
                controller.log(&format!("settings: couldn't start {bin}: {e}"));
            }
        }
        MenuAction::SetHeaderStyle(style) => {
            let _ = controller.do_action(&format!("header-style:{}", style.raw_value()));
        }
        #[cfg(target_os = "macos")]
        MenuAction::Quit => {
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                objc2_app_kit::NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
        // Off macOS the menus (and this dispatcher) are never installed, but
        // the match must still cover `Quit` there.
        #[cfg(not(target_os = "macos"))]
        MenuAction::Quit => {}
    }
}

/// `configSchemaJSON()` — the section-rule schema the `ws-settings` CLI reads
/// (`version`, sorted bool/color keys, numeric ranges, enum values).
fn config_schema() -> String {
    use serde_json::{json, Map, Value};

    /// `configEnumKeys` — the enum-checked keys (values via config_enum_keys).
    const ENUM_KEYS: [&str; 5] = ["header-style", "sort", "sort-order", "start-drawer", "type"];

    fn num(x: f64) -> Value {
        if x == x.round() && x.abs() < 9.0e15 {
            Value::Number(serde_json::Number::from(x as i64))
        } else {
            serde_json::Number::from_f64(x)
                .map(Value::Number)
                .unwrap_or_else(|| json!(0))
        }
    }

    let mut bools: Vec<&str> = app::config::CONFIG_BOOL_KEYS.to_vec();
    bools.sort_unstable();
    let mut colors: Vec<&str> = app::config::CONFIG_COLOR_KEYS.to_vec();
    colors.sort_unstable();

    let mut enums = Map::new();
    for key in ENUM_KEYS {
        let mut values = app::config::config_enum_keys(key).unwrap_or_default();
        values.sort_unstable();
        enums.insert(key.to_string(), json!(values));
    }

    let mut ranges = Map::new();
    for (key, lo, hi) in app::config::CONFIG_NUMBER_KEYS {
        ranges.insert(key.to_string(), json!([num(*lo), num(*hi)]));
    }

    let obj = json!({
        "boolKeys": bools,
        "colorKeys": colors,
        "enumKeys": enums,
        "numberRanges": ranges,
        "version": 1,
    });
    serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
}

/// `configCheckCLI`: the verdict as one JSON line on stdout — what
/// `settings_hub/schema.py` parses (`{"ok", "problem"}`, and for `--file`
/// `{"ok", "issues": [{line, message, fatal}]}`). The port printed plain text
/// on stderr, so ws-settings silently fell back to its partial checks.
fn config_check(argv: &[String]) -> i32 {
    use serde_json::{json, Value};
    if argv.len() == 2 && argv[0] == "--file" {
        let path = crate::app::config::expand_tilde(&argv[1]);
        let Ok(text) = std::fs::read_to_string(&path) else {
            let msg = format!("can't read {}", argv[1]);
            println!("{}", json!({"ok": false, "issues": [{"line": 0, "message": msg, "fatal": true}]}));
            return 1;
        };
        configure_helper();
        let issues = app::config::validate_config(&text);
        let list: Vec<Value> =
            issues.iter().map(|i| json!({"line": i.line, "message": i.message, "fatal": i.fatal})).collect();
        println!("{}", json!({"ok": !issues.iter().any(|i| i.fatal), "issues": list}));
        return 0;
    }
    if argv.len() != 3 {
        eprintln!("usage: kitchen-sink config-check SECTION KEY VALUE | --file PATH");
        return 2;
    }
    match app::config::config_value_problem(&argv[0], &argv[1], &argv[2]) {
        Some(problem) => println!("{}", json!({"ok": false, "problem": problem})),
        None => println!("{}", json!({"ok": true})),
    }
    0
}

fn print_usage() {
    println!("kitchen-sink (rust) — port scaffold");
    println!("usage: kitchen-sink [headless|window|files|jira|confluence|ai|terminal|ping|");
    println!("                     config-check SECTION KEY VALUE | config-check --file PATH | config-schema]");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_schema_matches_the_swift_shape() {
        let v: serde_json::Value = serde_json::from_str(&config_schema()).expect("schema json");
        assert_eq!(v["version"], serde_json::json!(1));

        let bools = v["boolKeys"].as_array().expect("boolKeys");
        assert!(bools.windows(2).all(|w| w[0].as_str() <= w[1].as_str()));
        assert!(bools.iter().any(|b| b == "shared-window"));

        let colors = v["colorKeys"].as_array().expect("colorKeys");
        assert!(colors.iter().any(|c| c == "terminal-background"));

        let ranges = v["numberRanges"].as_object().expect("numberRanges");
        assert_eq!(ranges["shared-width"], serde_json::json!([400, 8000]));
        assert_eq!(
            ranges["max-bytes"],
            serde_json::json!([1048576i64, 2147483648i64])
        );

        let enums = v["enumKeys"].as_object().expect("enumKeys");
        assert_eq!(
            enums["type"],
            serde_json::json!(["files", "list", "output", "shell"])
        );
        assert_eq!(enums["header-style"].as_array().unwrap().len(), 7);
    }
}

/// `[app] dock-icon = true` — show the Dock icon (default: menu bar only).
#[cfg(target_os = "macos")]
fn dock_icon_enabled(text: &str) -> bool {
    let mut in_app = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_app = t == "[app]";
        } else if in_app {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim().trim_matches('"') == "dock-icon" {
                    let v = v.split('#').next().unwrap_or("").trim().trim_matches('"');
                    return matches!(v.to_lowercase().as_str(), "true" | "yes" | "on" | "1");
                }
            }
        }
    }
    false
}
