//! The Paths tool panel — Wave 5 egui stub.
//!
//! Will drive the same pure model the AppKit half uses
//! ([`crate::views::paths`]) via its [`crate::views::paths::PathsSnapshot`] /
//! [`crate::views::paths::PathsEvent`] contract, mirroring `files.rs`'s shape:
//! pull a snapshot each frame, draw it with egui widgets, collect the user's
//! actions as events, and hand them to the model.

use crate::ui::theme::PopupColors;

use super::app::Pane;

/// The Paths panel. Not ported yet — a Wave 5 worker fills this in.
pub struct PathsPane;

impl PathsPane {
    pub fn build() -> Self {
        PathsPane
    }
}

impl Pane for PathsPane {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label("Paths — not ported yet");
    }
}
