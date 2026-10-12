//! The ONE colour conversion: [`crate::ui::theme::Rgba`] → `egui::Color32`.
//!
//! This is the mirror of `Rgba::to_nscolor` (which is macOS-only): components
//! are `f64` in `0.0..1.0`, converted here to the `u8` sRGB values egui wants.
//! Every colour the frontend draws goes through this one function, so the egui
//! palette tracks the `[theme]` tokens exactly as AppKit's `NSColor` does.

use egui::Color32;

/// `Rgba` → `egui::Color32` (the only colour conversion in the frontend).
pub fn c(rgba: crate::ui::theme::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (rgba.r * 255.0).round() as u8,
        (rgba.g * 255.0).round() as u8,
        (rgba.b * 255.0).round() as u8,
        (rgba.a * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Rgba;

    #[test]
    fn converts_opaque_components() {
        // `Color32` stores premultiplied *linear* bytes, so read back the
        // unmultiplied sRGB bytes (the same ones `to_nscolor` would produce).
        let c = c(Rgba::new(1.0, 0.0, 0.0, 1.0));
        assert_eq!(c.to_srgba_unmultiplied(), [255, 0, 0, 255]);
    }

    #[test]
    fn maps_black_and_white_exactly() {
        assert_eq!(c(Rgba::BLACK).to_srgba_unmultiplied(), [0, 0, 0, 255]);
        assert_eq!(c(Rgba::WHITE).to_srgba_unmultiplied(), [255, 255, 255, 255]);
    }

    #[test]
    fn preserves_alpha() {
        // Alpha is linear in `Color32`, so 0.5 * 255 = 127.5 → 128 round-trips.
        let c = c(Rgba::new(0.0, 0.0, 0.0, 0.5));
        assert_eq!(c.to_srgba_unmultiplied()[3], 128);
    }
}
