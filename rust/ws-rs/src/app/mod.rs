//! App host layer (mirrors `kitchen_sink.swift` top-level + `main.swift`).

pub mod cli;
pub mod config;
pub mod hotkey;
pub mod jira_poll;
pub mod host;
#[cfg(target_os = "macos")]
pub mod live_panes;
pub mod menu;
pub mod paths;
pub mod process_run;
pub mod python_helper;
pub mod registry;
pub mod socket;

/// Entry point. Phase 0 scaffold: no AppKit yet.
pub fn run() {
    println!("ws-rs {} (scaffold)", env!("CARGO_PKG_VERSION"));
}
