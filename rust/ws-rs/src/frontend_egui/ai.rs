//! The AI view — Wave 5 egui stub.
//!
//! Will drive the same pure model the AppKit half uses
//! ([`crate::views::ai_view`]) via its [`crate::views::ai_view::AISnapshot`] /
//! [`crate::views::ai_view::AIEvent`] contract, mirroring `files.rs`'s shape:
//! pull a snapshot each frame, draw it with egui widgets, collect the user's
//! actions as events, and hand them to the model.

use crate::ui::theme::PopupColors;

use super::app::Pane;

/// The AI pane. Not ported yet — a Wave 5 worker fills this in.
pub struct AiPane;

impl AiPane {
    pub fn build() -> Self {
        AiPane
    }
}

impl Pane for AiPane {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label("AI — not ported yet");
    }
}
