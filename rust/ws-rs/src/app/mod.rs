//! App host layer (mirrors `kitchen_sink.swift` top-level + `main.swift`).

pub mod config;
pub mod hotkey;
pub mod host;
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
