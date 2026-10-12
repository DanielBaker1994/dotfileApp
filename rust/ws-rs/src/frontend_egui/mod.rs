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
mod theme;

// Wave 5 stub panes — one file per worker, wired in `app::ensure_panes`.
mod ai;
mod compare;
mod confluence;
mod jira;
mod notes;
#[allow(dead_code)] // Wave 5 worker owns this; a tool panel, not yet wired into a SlotView.
mod paths;
mod switcher;
#[allow(dead_code)] // Wave 5 worker owns this (Tools + Setup); not yet wired into a SlotView.
mod tools;

pub use app::run;
