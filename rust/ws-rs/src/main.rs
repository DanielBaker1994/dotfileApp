//! kitchen-sink — the app's entry point (the former Swift `main.swift`).
//!
//! One-shot commands, the install check, the CLI client pass (`app::cli`),
//! then the daemon. The module tree mirrors the old Swift files.

#![allow(dead_code)]

mod app;
mod engines;
mod panes;
mod ui;
mod views;

use std::sync::{Arc, Mutex};

use app::host::SwitcherController;
use app::paths::Paths;
use app::python_helper::PythonHelper;
use app::socket::{CommandHandler, CommandServer, DaemonLock};

fn main() {
    std::env::set_var("PYTHONDONTWRITEBYTECODE", "1");

    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).cloned();

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
            "prose" => {
                configure_helper();
                let opts = views::notes::ProseLaunchOptions::parse(&args[2..]);
                if let Err(e) = views::notes::ProseProcess::run(&opts) {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
                // The floating window needs its own run loop (mirrors
                // `ProseProcess.run`'s standalone NSApplication).
                use objc2::MainThreadMarker;
                use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
                let mtm = MainThreadMarker::new().expect("main thread");
                let app = NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
                app::menu::install_main_menu(mtm, None);
                app.activate();
                app.run();
                return;
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
            run_daemon(paths.notes_socket_path(), mode);
        }
    }
}

/// `AppInstall.ensureHome()`: an installed app (not a repo build, not running
/// from the disk image) keeps `~/.config/kitchen-sink` set up for its version.
fn ensure_home(paths: &Paths) {
    use objc2_foundation::{NSBundle, NSString};
    let version = NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|v| v.to_string())
        .unwrap_or_default();
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

fn run_daemon(sock: String, mode: Option<String>) {
    let mut controller = SwitcherController::new_default();
    controller.reload_config();
    controller.register_views();
    let controller = Arc::new(controller);

    let _server = match CommandServer::start(&sock, controller.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kitchen-sink: cannot bind command socket {sock}: {e}");
            std::process::exit(1);
        }
    };

    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    // The Swift app is a regular app (menu bar + status item), not an
    // accessory — mirror that so the shared window is launchable.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app::menu::install_main_menu(mtm, None);
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

    eprintln!("kitchen-sink (rust) daemon: socket {sock}");
    app.run();
}

/// Install the menu-bar status item from the ported menu spec. The handle is
/// leaked on purpose: the status item lives as long as the process.
fn install_status_item(mtm: objc2::MainThreadMarker, controller: &Arc<SwitcherController>) {
    use app::menu::{build_menu_spec, install_status_item, MenuSettings};
    let settings = controller.settings();
    let menu_settings = MenuSettings {
        hide_on_focus_loss: settings.hide_on_focus_loss,
        header_style: crate::ui::theme::HeaderStyle::from_raw(settings.header_style.raw())
            .unwrap_or_default(),
        ..MenuSettings::default()
    };
    let spec = build_menu_spec(&menu_settings);
    let c = controller.clone();
    let handle = install_status_item(mtm, &spec, move |action| status_menu_action(&c, action));
    std::mem::forget(handle);
}

/// Route the status-menu clicks onto the controller (a growing subset of the
/// Swift `MenuTarget` handlers; anything else is a no-op for now).
fn status_menu_action(controller: &Arc<SwitcherController>, action: app::menu::MenuAction) {
    use app::menu::MenuAction;
    use app::registry::SlotView;
    match action {
        MenuAction::ToggleNotes => controller.hotkey(SlotView::Notes),
        MenuAction::ToggleJiraWindow => controller.hotkey(SlotView::Jira),
        MenuAction::OpenWindow => controller.toggle(),
        MenuAction::ToggleTerminal => {
            let _ = controller.do_action("toggle-terminal");
        }
        MenuAction::CloseWindow => controller.hide("menu"),
        MenuAction::ToggleHideOnFocusLoss => {
            let (on, _) = controller.focus_loss_settings();
            controller.set_hide_on_focus_loss(!on, true);
        }
        MenuAction::SetHeaderStyle(style) => {
            let _ = controller.do_action(&format!("header-style:{}", style.raw_value()));
        }
        MenuAction::Quit => {
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                objc2_app_kit::NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
        _ => {}
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

fn config_check(argv: &[String]) -> i32 {
    if argv.len() < 3 {
        eprintln!("usage: kitchen-sink config-check SECTION KEY VALUE");
        return 2;
    }
    match app::config::config_value_problem(&argv[0], &argv[1], &argv[2]) {
        Some(problem) => {
            eprintln!("{problem}");
            1
        }
        None => 0,
    }
}

fn print_usage() {
    println!("kitchen-sink (rust) — port scaffold");
    println!("usage: kitchen-sink [window|notes|files|jira|confluence|ai|compare|terminal|ping|");
    println!("                     config-check SECTION KEY VALUE | config-schema]");
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
            serde_json::json!(["files", "list", "note", "output", "shell"])
        );
        assert_eq!(enums["header-style"].as_array().unwrap().len(), 7);
    }
}
