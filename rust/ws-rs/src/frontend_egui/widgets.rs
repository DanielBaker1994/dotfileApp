//! The shared egui widgets: the pieces AppKit draws once through
//! `ui::style` / `ui::outline_button` / `ui::sidebar`, so no pane
//! hand-rolls them differently. Mirrors:
//!
//! * `section_label` — the `SidebarView` uppercase section head.
//! * `badge` — the tinted inspector chip (AppKit `capsule_chip`-toned).
//! * `capsule` / `capsule_accent` — the `OutlineButton` one-shot tones.
//! * `rail_button` — the collapse-to-rail toggle (the sidebar's bottom-left
//!   button and Jira's inspector rail strip; egui has no SF Symbols, so the
//!   collapsed rail is the compact expand strip rather than AppKit's icon
//!   column — same state, same persisted key, simpler rendering).

use crate::ui::theme::{PopupColors, Rgba};
use egui::{Color32, CornerRadius, RichText, Response, Stroke, Ui};

use super::theme::c;

/// The expanded sidebar width (the AppKit panes' `sidebar-width` default).
pub const SIDEBAR_W: f32 = 200.0;
/// The Jira ticket panel's expanded width (the `inspector-width` default is
/// 264; the egui table needs more room, so the strip starts wider).
pub const INSPECTOR_W: f32 = 300.0;
/// The collapsed rail strip (the mirror of `SidebarView::RAIL_WIDTH` /
/// `jira_view::INSPECTOR_RAIL_W`): just the expand button rides here.
pub const RAIL_W: f32 = 44.0;

/// The uppercase section header every sidebar draws (`SidebarView` heads:
/// MY WORK / PLACES / RULES …). Deduplicates the five panes' inline
/// `RichText::new(t.to_uppercase()).color(dim).small()` blocks.
pub fn section_label(ui: &mut Ui, text: &str, colors: &PopupColors) {
    ui.label(RichText::new(text.to_uppercase()).color(c(colors.dim)).small());
}

/// A tinted status badge (the `JiraInspectorView` chips: To Do / Highest).
pub fn badge(ui: &mut Ui, text: &str, tint: Rgba) {
    ui.label(
        RichText::new(text)
            .color(c(tint))
            .background_color(c(tint.with_alpha(0.18)))
            .small(),
    );
}

/// The Neutral one-shot capsule — the egui mirror of
/// [`crate::ui::outline_button::Tone::Neutral`] (the "Load more" / "Copy
/// CQL" role): pill rounding, text-tinted outline, no system bezel.
pub fn capsule(ui: &mut Ui, label: &str, colors: &PopupColors) -> Response {
    capsule_e(ui, label, colors, true)
}

/// [`capsule`] in the single-toggle role (`OutlineButton`'s `on` state):
/// the filled selected-chip language when on, the Neutral outline when off
/// (the Files toolbar "Pinned" case).
pub fn capsule_toggle(ui: &mut Ui, label: &str, on: bool, colors: &PopupColors) -> Response {
    if !on {
        return capsule(ui, label, colors);
    }
    ui.add(
        egui::Button::new(RichText::new(label).color(c(colors.accent)))
            .corner_radius(CornerRadius::same(u8::MAX))
            .fill(c(colors.accent.with_alpha(0.22)))
            .stroke(Stroke::new(1.0, c(colors.accent.with_alpha(0.65)))),
    )
}

/// [`capsule`] with an explicit enabled state (dimmed like
/// `OutlineButton::set_enabled`).
pub fn capsule_e(
    ui: &mut Ui,
    label: &str,
    colors: &PopupColors,
    enabled: bool,
) -> Response {
    let text = RichText::new(label).color(c(if enabled {
        colors.text.with_alpha(0.85)
    } else {
        colors.dim.with_alpha(0.6)
    }));
    ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .corner_radius(CornerRadius::same(u8::MAX))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, c(colors.text.with_alpha(0.25)))),
    )
}

/// The Accent one-shot capsule (the `Tone::Accent` role — "Run" /
/// "Open full detail"): accent-tinted outline + text.
pub fn capsule_accent(ui: &mut Ui, label: &str, colors: &PopupColors) -> Response {
    let text = RichText::new(label).color(c(colors.accent));
    ui.add(
        egui::Button::new(text)
            .corner_radius(CornerRadius::same(u8::MAX))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, c(colors.accent.with_alpha(0.65)))),
    )
}

/// The collapse-to-rail toggle: `«` on an expanded pane, `»` on the rail
/// strip. Returns true when clicked (the owner flips its rail flag and
/// persists the `sidebarRail.<key>` setting).
pub fn rail_button(ui: &mut Ui, collapsed: bool, colors: &PopupColors) -> bool {
    let (glyph, tip) = if collapsed {
        ("»", "Show the sidebar  ⌃B B")
    } else {
        ("«", "Collapse the sidebar  ⌃B B")
    };
    capsule(ui, glyph, colors).on_hover_text(tip).clicked()
}
