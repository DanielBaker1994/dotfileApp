//! The Tools panels (prettyprint / filefast / health-checks) and the Setup
//! view — Wave 5 egui stub.
//!
//! Will drive the same pure models the AppKit half uses via their
//! Snapshot/Event contracts, mirroring `files.rs`'s shape:
//! [`crate::views::tools::ToolsSnapshot`] / [`crate::views::tools::ToolsEvent`]
//! for the tool panels, and [`crate::views::setup::SetupSnapshot`] /
//! [`crate::views::setup::SetupEvent`] for the Setup view.

use crate::ui::theme::PopupColors;

use super::app::Pane;

/// The Tools panel host. Not ported yet — a Wave 5 worker fills this in.
pub struct ToolsPane;

impl ToolsPane {
    pub fn build() -> Self {
        ToolsPane
    }
}

impl Pane for ToolsPane {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label("Tools — not ported yet");
    }
}
