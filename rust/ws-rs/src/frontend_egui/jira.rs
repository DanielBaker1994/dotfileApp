//! The Jira view — Wave 5 egui stub.
//!
//! Will drive the same pure model the AppKit half uses
//! ([`crate::views::jira_view`]) via its
//! [`crate::views::jira_view::JiraSnapshot`] /
//! [`crate::views::jira_view::JiraEvent`] contract, mirroring `files.rs`'s
//! shape: pull a snapshot each frame, draw it with egui widgets, collect the
//! user's actions as events, and hand them to the model.

use crate::ui::theme::PopupColors;

use super::app::Pane;

/// The Jira pane. Not ported yet — a Wave 5 worker fills this in.
pub struct JiraPane;

impl JiraPane {
    pub fn build() -> Self {
        JiraPane
    }
}

impl Pane for JiraPane {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label("Jira — not ported yet");
    }
}
