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

pub use app::run;
