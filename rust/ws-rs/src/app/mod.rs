//! App host layer (mirrors `kitchen_sink.swift` top-level + `main.swift`).

pub mod cli;
pub use ks_core::app::config;
pub use ks_core::app::hotkey;
pub mod jira_poll;
pub mod host;
#[cfg(target_os = "macos")]
pub use ks_core::app::live_panes;
pub use ks_core::app::logging;
pub use ks_core::app::menu;
pub use ks_core::app::pathfmt;
pub use ks_core::app::paths;
pub use ks_core::app::process_run;
pub use ks_core::app::python_helper;
pub mod quick;
pub use ks_core::app::registry;
pub use ks_core::app::socket;
pub mod synth_input;

/// Entry point. Phase 0 scaffold: no AppKit yet.
pub fn run() {
    println!("ws-rs {} (scaffold)", env!("CARGO_PKG_VERSION"));
}
