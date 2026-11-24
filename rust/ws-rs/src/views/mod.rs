//! Slot views (mirrors the per-view Swift files).

// Modules moved to `ks-core` are re-exported so `crate::<mod>::…` paths stay valid;
// not every re-export is used by every target.
#![allow(unused_imports)]

pub mod ai;
pub mod ai_view;
pub mod confluence;
pub mod confluence_view;
pub use ks_core::views::file_popup;
pub use ks_core::views::files;
pub use ks_core::views::files_view;
pub mod jira;
pub mod jira_config;
pub mod jira_view;
pub mod jira_search_bar;
pub mod pane_find;
pub use ks_core::views::paths;
pub use ks_core::views::screenshot;
pub mod setup;
pub use ks_core::views::switcher;
pub mod terminal;
pub use ks_core::views::tools;
