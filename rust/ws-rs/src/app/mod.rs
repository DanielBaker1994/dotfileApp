//! App host layer (mirrors `kitchen_sink.swift` top-level + `main.swift`).

// Modules moved to `ks-core` are re-exported so `crate::<mod>::…` paths stay valid;
// not every re-export is used by every target.
#![allow(unused_imports)]

pub use ks_core::app::cli;
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
pub use ks_core::app::quick;
pub use ks_core::app::registry;
pub use ks_core::app::socket;
pub mod synth_input;

