//! Foundation-only engines + Python-facing facades (mirrors the leaf Swift
//! files). Most delegate to `pylib/` through the helper.

pub mod ai_format;
pub mod ansi_render;
pub mod config_text;
pub mod file_ops;
pub mod list_filter;
pub mod nvim_rpc;
pub mod pane_shot;
pub mod path_shelf;
pub mod recent_files;
pub mod screenshot_annotations;
pub mod screenshot_text;
pub mod switcher_status;
pub mod text_edit_keys;
