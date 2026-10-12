//! The switcher / command-palette overlay — Wave 5 egui stub.
//!
//! An overlay, not a [`crate::app::registry::SlotView`] pane: the host draws
//! it last, on top of the active pane. Will drive
//! [`crate::views::switcher::SwitcherSnapshot`] /
//! [`crate::views::switcher::SwitcherEvent`] when ported.

/// The palette/switcher overlay. Not ported yet — draws nothing.
pub struct SwitcherOverlay;

impl SwitcherOverlay {
    pub fn build() -> Self {
        SwitcherOverlay
    }

    /// Draw the overlay last, over the active pane (stub: draws nothing).
    pub fn ui(&mut self, _ctx: &egui::Context) {}
}
