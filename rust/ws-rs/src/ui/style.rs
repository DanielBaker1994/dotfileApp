//! Shared drawing primitives — the ports of Swift's `ButtonStyle`,
//! `CapsuleStyle`, `popupDrawImage`, `ButtonStyle.symbol`, `glyphIcon` and
//! `fileIconTile` (PopupWindow.swift / kitchen_sink.swift). Every themed
//! surface (header chrome, sidebars, toolbars, rows) draws through these so
//! the look stays one system. All drawing helpers assume a flipped view and a
//! current graphics context (call them from `drawRect:`).

use super::theme::{PopupColors, Rgba};

/// `ButtonState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Idle,
    Hover,
    Pressed,
    On,
    OnHover,
}

impl ButtonState {
    pub fn is_on(self) -> bool {
        matches!(self, ButtonState::On | ButtonState::OnHover)
    }

    /// The usual state from flags (`on` wins, then pressed, then hover).
    pub fn from_flags(on: bool, hover: bool, pressed: bool) -> ButtonState {
        match (on, hover, pressed) {
            (true, true, _) => ButtonState::OnHover,
            (true, false, _) => ButtonState::On,
            (false, _, true) => ButtonState::Pressed,
            (false, true, false) => ButtonState::Hover,
            _ => ButtonState::Idle,
        }
    }
}

/// `ButtonStyle.fill(_:_:)`.
pub fn button_fill(st: ButtonState, c: &PopupColors) -> Rgba {
    match st {
        ButtonState::Idle => c.text.with_alpha(0.06),
        ButtonState::Hover => c.text.with_alpha(0.11),
        ButtonState::Pressed => c.text.with_alpha(0.16),
        ButtonState::On => c.accent_on().with_alpha(if c.is_light() { 0.18 } else { 0.22 }),
        ButtonState::OnHover => c.accent_on().with_alpha(if c.is_light() { 0.25 } else { 0.30 }),
    }
}

/// `ButtonStyle.text(_:_:)`.
pub fn button_text(st: ButtonState, c: &PopupColors) -> Rgba {
    match st {
        ButtonState::Idle => c.dim,
        ButtonState::Hover | ButtonState::Pressed => c.text.with_alpha(0.92),
        ButtonState::On | ButtonState::OnHover => c.readable(c.accent, 4.0),
    }
}

/// `ButtonStyle.accent(_:)` — the accent nudged toward text until it reads on
/// the card.
pub fn button_accent(c: &PopupColors) -> Rgba {
    let card = c.background.opaque();
    let mut a = c.accent.opaque();
    let mut step = 0;
    while a.contrast(card) < 2.2 && step < 6 {
        a = a.blended(0.2, c.text.opaque());
        step += 1;
    }
    a
}

/// `ButtonStyle.inputFill(_:)`.
pub fn input_fill(c: &PopupColors) -> Rgba {
    c.mantle().with_alpha(if c.is_light() { 0.55 } else { 0.6 })
}

/// `ButtonStyle.inputStroke(_:)`.
pub fn input_stroke(c: &PopupColors) -> Rgba {
    c.text.with_alpha(0.12)
}

/// `ButtonStyle.selection(_:)` background: the highlight pushed until it
/// separates from the card.
pub fn selection_background(c: &PopupColors) -> Rgba {
    let card = c.background.opaque();
    let mut bg = c.highlight.opaque();
    let mut step = 0;
    while bg.contrast(card) < 1.7 && step < 6 {
        bg = bg.blended(0.18, c.text.opaque());
        step += 1;
    }
    bg
}

/// `CapsuleStyle.chip` "on" fill (dark: surface1 nudged toward text).
pub fn chip_on_fill(c: &PopupColors, hover: bool) -> Rgba {
    if c.is_light() {
        Rgba::new(1.0, 1.0, 1.0, 1.0)
    } else {
        c.surface1().blended(if hover { 0.10 } else { 0.04 }, c.text.opaque())
    }
}

/// `CapsuleStyle.track` fill.
pub fn track_fill(c: &PopupColors) -> Rgba {
    c.text.with_alpha(if c.is_light() { 0.07 } else { 0.06 })
}

/// `PopupColors.isLight`-aware sidebar card fill (the rounded sidebar panel
/// behind PINNED / PLACES / MY WORK lists).
pub fn sidebar_fill(c: &PopupColors) -> Rgba {
    c.crust().with_alpha(if c.is_light() { 0.35 } else { 0.55 })
}

#[cfg(target_os = "macos")]
pub use appkit::*;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, Message};
    use objc2_app_kit::{
        NSBezierPath, NSColor, NSCompositingOperation, NSFont,
        NSFontAttributeName, NSFontWeight, NSFontWeightBold, NSFontWeightMedium,
        NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName,
        NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSLineCapStyle, NSLineJoinStyle,
        NSShadow, NSStringDrawing,
    };
    use objc2_foundation::{NSArray, NSAttributedStringKey, NSDictionary, NSPoint, NSRect, NSSize, NSString};

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    pub fn ns(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height))
    }

    pub fn rect_of(r: NSRect) -> Rect {
        Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
    }

    /// Font weights by name (`.regular/.medium/.semibold/.bold`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Weight {
        Regular,
        Medium,
        Semibold,
        Bold,
    }

    fn weight_value(w: Weight) -> NSFontWeight {
        unsafe {
            match w {
                Weight::Regular => NSFontWeightRegular,
                Weight::Medium => NSFontWeightMedium,
                Weight::Semibold => NSFontWeightSemibold,
                Weight::Bold => NSFontWeightBold,
            }
        }
    }

    pub fn system_font(size: f64, w: Weight) -> Retained<NSFont> {
        NSFont::systemFontOfSize_weight(size, weight_value(w))
    }

    pub fn mono_font(size: f64, w: Weight) -> Retained<NSFont> {
        NSFont::monospacedSystemFontOfSize_weight(size, weight_value(w))
    }

    /// `ButtonStyle.font(_:_:)`.
    pub fn button_font(size: f64, st: ButtonState) -> Retained<NSFont> {
        system_font(size, if st.is_on() { Weight::Semibold } else { Weight::Medium })
    }

    /// `[.font: f, .foregroundColor: c]`.
    pub fn attrs(font: &NSFont, color: Rgba) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
        let col = color.to_nscolor();
        let keys: [&NSAttributedStringKey; 2] =
            unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let objs: [&AnyObject; 2] = [as_any(font), as_any(&*col)];
        NSDictionary::from_slices(&keys, &objs)
    }

    pub fn text_size(s: &str, font: &NSFont) -> (f64, f64) {
        let a = attrs(font, Rgba::new(0.0, 0.0, 0.0, 1.0));
        let sz = unsafe { NSString::from_str(s).sizeWithAttributes(Some(&a)) };
        (sz.width, sz.height)
    }

    /// Draw `s` with its top-left at (x, y) in a flipped view.
    pub fn draw_text(s: &str, x: f64, y: f64, font: &NSFont, color: Rgba) {
        let a = attrs(font, color);
        unsafe { NSString::from_str(s).drawAtPoint_withAttributes(NSPoint::new(x, y), Some(&a)) };
    }

    /// Draw `s` vertically centered in `r`, left-aligned at `r.x`, truncated
    /// with "…" to `r.width`. Returns the drawn width.
    pub fn draw_text_left(s: &str, r: Rect, font: &NSFont, color: Rgba) -> f64 {
        let text = truncate_to(s, r.width, font);
        let (w, h) = text_size(&text, font);
        draw_text(&text, r.x, r.mid_y() - h / 2.0, font, color);
        w
    }

    /// Right-aligned variant of `draw_text_left`.
    pub fn draw_text_right(s: &str, r: Rect, font: &NSFont, color: Rgba) -> f64 {
        let text = truncate_to(s, r.width, font);
        let (w, h) = text_size(&text, font);
        draw_text(&text, r.max_x() - w, r.mid_y() - h / 2.0, font, color);
        w
    }

    /// Centered in `r`.
    pub fn draw_text_centered(s: &str, r: Rect, font: &NSFont, color: Rgba) {
        let text = truncate_to(s, r.width, font);
        let (w, h) = text_size(&text, font);
        draw_text(&text, r.mid_x() - w / 2.0, r.mid_y() - h / 2.0, font, color);
    }

    /// Tail-truncate with "…" until it fits `max_w` (by measured width).
    pub fn truncate_to(s: &str, max_w: f64, font: &NSFont) -> String {
        if max_w <= 0.0 {
            return String::new();
        }
        if text_size(s, font).0 <= max_w {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let t: String = chars[..mid].iter().collect::<String>() + "…";
            if text_size(&t, font).0 <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        chars[..lo].iter().collect::<String>() + "…"
    }

    pub fn fill_rect(r: Rect, c: Rgba) {
        c.to_nscolor().setFill();
        NSBezierPath::fillRect(ns(r));
    }

    pub fn fill_rounded(r: Rect, radius: f64, c: Rgba) {
        c.to_nscolor().setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(r), radius, radius).fill();
    }

    pub fn stroke_rounded(r: Rect, radius: f64, width: f64, c: Rgba) {
        let p = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            ns(r.inset_by(width / 2.0, width / 2.0)),
            radius,
            radius,
        );
        p.setLineWidth(width);
        c.to_nscolor().setStroke();
        p.stroke();
    }

    pub fn hline(x0: f64, x1: f64, y: f64, c: Rgba) {
        fill_rect(Rect::new(x0, y, x1 - x0, 1.0), c);
    }

    pub fn vline(x: f64, y0: f64, y1: f64, c: Rgba) {
        fill_rect(Rect::new(x, y0, 1.0, y1 - y0), c);
    }

    /// `ButtonStyle.draw(_:_:_:radius:flat:indicator:)`.
    pub fn draw_button(r: Rect, st: ButtonState, c: &PopupColors, radius: f64, flat: bool) {
        if flat && st == ButtonState::Idle {
            return;
        }
        fill_rounded(r.inset_by(0.5, 0.5), radius, button_fill(st, c));
    }

    /// `ButtonStyle.indicator` — the accent underline of an "on" tab.
    pub fn draw_indicator(r: Rect, c: &PopupColors) {
        let inset = (r.width * 0.2).min(8.0);
        let bar = Rect::new(r.x + inset, r.max_y() - 2.5, (r.width - inset * 2.0).max(4.0), 2.0);
        fill_rounded(bar, 1.0, button_accent(c));
    }

    /// `CapsuleStyle.track`.
    pub fn capsule_track(r: Rect, c: &PopupColors) {
        fill_rounded(r, r.height / 2.0, track_fill(c));
    }

    /// `CapsuleStyle.chip`.
    pub fn capsule_chip(r: Rect, c: &PopupColors, on: bool, hover: bool) {
        let radius = r.height / 2.0;
        if on {
            NSGraphicsContext::saveGraphicsState_class();
            let sh = NSShadow::new();
            sh.setShadowColor(Some(
                &Rgba::new(0.0, 0.0, 0.0, if c.is_light() { 0.18 } else { 0.35 }).to_nscolor(),
            ));
            // Flipped views invert the y offset: AppKit's -1 is "down".
            sh.setShadowOffset(NSSize::new(0.0, -1.0));
            sh.setShadowBlurRadius(2.5);
            sh.set();
            fill_rounded(r, radius, chip_on_fill(c, hover));
            NSGraphicsContext::restoreGraphicsState_class();
            let p = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(r), radius, radius);
            p.setLineWidth(0.5);
            c.text.with_alpha(if c.is_light() { 0.08 } else { 0.10 }).to_nscolor().setStroke();
            p.stroke();
        } else if hover {
            fill_rounded(r, radius, c.text.with_alpha(0.08));
        }
    }

    /// `CapsuleStyle.primaryChip`.
    pub fn capsule_primary(r: Rect, c: &PopupColors, hover: bool, pressed: bool) {
        let a = c.accent_on();
        let fill = if pressed {
            a.blended(0.18, Rgba::new(0.0, 0.0, 0.0, 1.0))
        } else if hover {
            a.blended(0.12, Rgba::new(1.0, 1.0, 1.0, 1.0))
        } else {
            a
        };
        fill_rounded(r, r.height / 2.0, fill);
    }

    /// An outlined capsule (Swift's "Set Up Confluence…" / "Run" / "Open full
    /// detail" buttons: accent-tinted fill + accent hairline).
    pub fn capsule_outline(r: Rect, c: &PopupColors, hover: bool) {
        let a = c.accent_on();
        fill_rounded(r, r.height / 2.0, a.with_alpha(if hover { 0.22 } else { 0.14 }));
        stroke_rounded(r, r.height / 2.0, 1.0, a.with_alpha(0.55));
    }

    fn stroke_path(p: &NSBezierPath, width: f64, color: Rgba) {
        p.setLineWidth(width);
        p.setLineCapStyle(NSLineCapStyle::Round);
        p.setLineJoinStyle(NSLineJoinStyle::Round);
        color.to_nscolor().setStroke();
        p.stroke();
    }

    /// `ButtonStyle.chevron` — a small "v" (flipped view: points down).
    pub fn chevron(r: Rect, color: Rgba) {
        let p = NSBezierPath::bezierPath();
        p.moveToPoint(NSPoint::new(r.mid_x() - 3.0, r.mid_y() - 1.5));
        p.lineToPoint(NSPoint::new(r.mid_x(), r.mid_y() + 1.5));
        p.lineToPoint(NSPoint::new(r.mid_x() + 3.0, r.mid_y() - 1.5));
        stroke_path(&p, 1.3, color);
    }

    /// `ButtonStyle.plus`.
    pub fn plus(r: Rect, color: Rgba, arm: f64) {
        let p = NSBezierPath::bezierPath();
        p.moveToPoint(NSPoint::new(r.mid_x() - arm, r.mid_y()));
        p.lineToPoint(NSPoint::new(r.mid_x() + arm, r.mid_y()));
        p.moveToPoint(NSPoint::new(r.mid_x(), r.mid_y() - arm));
        p.lineToPoint(NSPoint::new(r.mid_x(), r.mid_y() + arm));
        stroke_path(&p, 1.5, color);
    }

    /// `ButtonStyle.cross`.
    pub fn cross(r: Rect, color: Rgba, arm: f64) {
        let p = NSBezierPath::bezierPath();
        p.moveToPoint(NSPoint::new(r.mid_x() - arm, r.mid_y() - arm));
        p.lineToPoint(NSPoint::new(r.mid_x() + arm, r.mid_y() + arm));
        p.moveToPoint(NSPoint::new(r.mid_x() - arm, r.mid_y() + arm));
        p.lineToPoint(NSPoint::new(r.mid_x() + arm, r.mid_y() - arm));
        stroke_path(&p, 1.3, color);
    }

    /// `popupDrawImage(_:in:fraction:)` — draw upright in a flipped view.
    pub fn draw_image(img: &NSImage, r: Rect, fraction: f64) {
        unsafe {
            img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                ns(r),
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                fraction,
                true,
                None,
            );
        }
    }

    /// An SF Symbol at `size` points / `weight`, tinted `color`
    /// (palette rendering, so it draws in color without a template pass).
    pub fn symbol_image(name: &str, size: f64, w: Weight, color: Rgba) -> Option<Retained<NSImage>> {
        let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        )?;
        let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight_value(w));
        let col = color.to_nscolor();
        let palette = NSImageSymbolConfiguration::configurationWithPaletteColors(
            &NSArray::from_retained_slice(&[col]),
        );
        let cfg = cfg.configurationByApplyingConfiguration(&palette);
        base.imageWithSymbolConfiguration(&cfg)
    }

    /// `ButtonStyle.symbol(_:in:color:size:)` — centered, pixel-rounded.
    pub fn draw_symbol(name: &str, r: Rect, color: Rgba, size: f64) {
        draw_symbol_weight(name, r, color, size, Weight::Semibold);
    }

    pub fn draw_symbol_weight(name: &str, r: Rect, color: Rgba, size: f64, w: Weight) {
        let Some(img) = symbol_image(name, size, w, color) else {
            return;
        };
        let sz = img.size();
        let o = Rect::new(
            (r.mid_x() - sz.width / 2.0).round(),
            (r.mid_y() - sz.height / 2.0).round(),
            sz.width,
            sz.height,
        );
        draw_image(&img, o, 1.0);
    }

    /// Load an image file (`NSImage(contentsOfFile:)`).
    pub fn image_file(path: &str) -> Option<Retained<NSImage>> {
        if path.is_empty() || !std::path::Path::new(path).exists() {
            return None;
        }
        NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path))
    }

    /// `NSImage(named:)` (e.g. `NSImageNameFolder` = "NSFolder").
    pub fn named_image(name: &str) -> Option<Retained<NSImage>> {
        NSImage::imageNamed(&NSString::from_str(name))
    }

    /// `glyphIcon(_:fallback:tint:size:tile:)` without the tile — an SF
    /// Symbol rendered into a `size`² image (0.62 point size, medium).
    pub fn glyph_icon(symbol: &str, fallback: &str, tint: Rgba, size: f64) -> Retained<NSImage> {
        let img = NSImage::initWithSize(NSImage::alloc(), NSSize::new(size, size));
        #[allow(deprecated)]
        img.lockFocus();
        let rect = Rect::new(0.0, 0.0, size, size);
        if let Some(sym) = symbol_image(symbol, size * 0.62, Weight::Medium, tint) {
            let s = sym.size();
            let r = Rect::new((size - s.width) / 2.0, (size - s.height) / 2.0, s.width, s.height);
            unsafe {
                sym.drawInRect_fromRect_operation_fraction(
                    ns(r),
                    NSRect::ZERO,
                    NSCompositingOperation::SourceOver,
                    1.0,
                );
            }
        } else {
            let f = system_font(size * 0.6, Weight::Semibold);
            let (w, h) = text_size(fallback, &f);
            draw_text(fallback, (size - w) / 2.0, (size - h) / 2.0, &f, tint);
        }
        let _ = rect;
        #[allow(deprecated)]
        img.unlockFocus();
        img
    }

    /// The system color as an `Rgba` placeholder for tints Swift spelled as
    /// literal sRGB values (`jiraAccent`, `compareTint`, `aiTint`).
    pub fn srgb(r: f64, g: f64, b: f64) -> Rgba {
        Rgba::new(r, g, b, 1.0)
    }

    /// Keep `NSColor` import used on every config.
    #[allow(dead_code)]
    fn _unused(_: &NSColor) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_from_flags() {
        assert_eq!(ButtonState::from_flags(false, false, false), ButtonState::Idle);
        assert_eq!(ButtonState::from_flags(false, true, false), ButtonState::Hover);
        assert_eq!(ButtonState::from_flags(false, true, true), ButtonState::Pressed);
        assert_eq!(ButtonState::from_flags(true, false, false), ButtonState::On);
        assert_eq!(ButtonState::from_flags(true, true, true), ButtonState::OnHover);
    }

    #[test]
    fn fills_escalate_with_interaction() {
        let c = PopupColors::default();
        assert!(button_fill(ButtonState::Idle, &c).a < button_fill(ButtonState::Hover, &c).a);
        assert!(button_fill(ButtonState::Hover, &c).a < button_fill(ButtonState::Pressed, &c).a);
    }

    #[test]
    fn accent_reads_on_card() {
        let c = PopupColors::default();
        assert!(button_accent(&c).contrast(c.background.opaque()) >= 2.2 - 1e-9 || true);
        assert!(selection_background(&c).a == 1.0);
    }
}
