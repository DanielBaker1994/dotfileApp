//! The Notes view — Wave 5 egui stub.
//!
//! Will drive the same pure model the AppKit half uses
//! ([`crate::views::notes`]) via its [`crate::views::notes::NotesSnapshot`] /
//! [`crate::views::notes::NotesEvent`] contract, mirroring `files.rs`'s shape:
//! pull a snapshot each frame, draw it with egui widgets, collect the user's
//! actions as events, and hand them to the model.

use crate::ui::theme::PopupColors;

use super::app::Pane;

/// The Notes pane. Not ported yet — a Wave 5 worker fills this in.
pub struct NotesPane;

impl NotesPane {
    pub fn build() -> Self {
        NotesPane
    }
}

impl Pane for NotesPane {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label("Notes — not ported yet");
    }
}
