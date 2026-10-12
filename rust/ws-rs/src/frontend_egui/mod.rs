//! The egui frontend (Wave 4+) — the non-macOS UI, and the `WS_FRONTEND=egui`
//! dev switch on macOS.
//!
//! It drives the same toolkit-neutral core the AppKit half uses
//! ([`crate::app::host::SwitcherController`], the pure
//! [`crate::views::files::FileBrowser`] model and its
//! [`crate::views::files_view::FilesSnapshot`] / `FilesEvent` contract), so the
//! macOS path stays byte-identical. The whole module is compiled only when it
//! is needed: `#[cfg(any(not(target_os = "macos"), feature = "egui"))]` in
//! `main.rs`.

mod app;
mod files;
mod keys;
mod terminal;
mod theme;

// Wave 5 stub panes — one file per worker, wired in `app::ensure_panes`
// (files/notes/jira/confluence/ai/compare) or as floating tool windows in
// `app.rs` (paths in the Paths window, tools in the Tools window).
mod ai;
mod compare;
mod confluence;
mod jira;
mod notes;
mod paths;
mod switcher;
#[allow(dead_code)] // `SetupPane` is not wired into a SlotView yet; `ToolsPane` is (app.rs).
mod tools;

pub use app::run;
