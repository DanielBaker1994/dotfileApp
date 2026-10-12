//! The Compare view — Wave 5 egui stub.
//!
//! Will drive the same pure model the AppKit half uses
//! ([`crate::views::compare_view`]) via its
//! [`crate::views::compare_view::CompareSnapshot`] /
//! [`crate::views::compare_view::CompareEvent`] contract, mirroring
//! `files.rs`'s shape: pull a snapshot each frame, draw it with egui widgets,
//! collect the user's actions as events, and hand them to the model.

use crate::ui::theme::PopupColors;

use super::app::Pane;

/// The Compare pane. Not ported yet — a Wave 5 worker fills this in.
pub struct ComparePane;

impl ComparePane {
    pub fn build() -> Self {
        ComparePane
    }
}

impl Pane for ComparePane {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label("Compare — not ported yet");
    }
}
