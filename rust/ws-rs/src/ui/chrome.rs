//! `PopupChrome` header, ported from `PopupWindow.swift`.
//!
//! The geometry and the per-[`HeaderStyle`] background math are plain Rust
//! (rects + theme tokens) so they are unit-testable; the AppKit side
//! (`define_class!` over `NSView`) only consumes those values in `drawRect:`.
//!
//! First cut: close-button / icon / nav geometry, the header-title rule, the
//! background paint ops for all seven styles, `drawRect:` for the solid
//! styles (fills + bottom rule; gradients via `NSGradient`), the close glyph,
//! and `redrawAll`. Nav-icon/label text drawing is a later cut.

use crate::ui::theme::{current_header_style, HeaderStyle, PopupColors, PopupTone, Rgba};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSImage;

/// A plain, flipped (top-left origin) rectangle, matching `NSRect` in the
/// flipped `PopupChrome`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };

    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Rect { x, y, width, height }
    }

    pub fn min_x(&self) -> f64 {
        self.x
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }
    pub fn min_y(&self) -> f64 {
        self.y
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.max_x() && y >= self.y && y < self.max_y()
    }

    pub fn inset_by(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width - dx * 2.0, self.height - dy * 2.0)
    }
}

/// `PopupChrome.closeButtonRect` geometry constants.
pub const CLOSE_BUTTON_SIZE: f64 = 22.0;
pub const CLOSE_BUTTON_INSET: f64 = 6.0;
pub const ICON_BUTTON_WIDTH: f64 = 40.0;
pub const ICON_BUTTON_HEIGHT: f64 = 22.0;
pub const NAV_WIDTH: f64 = 32.0;
pub const NAV_HEIGHT: f64 = 24.0;
pub const NAV_GAP: f64 = 2.0;

/// `closeButtonRect`: a 22×22 square at the left inset, vertically centered in
/// the header; `.zero` when the header has no close button or no height.
pub fn close_button_rect(
    header_close_button: bool,
    header_height: f64,
    button_width: f64,
    inset: f64,
) -> Rect {
    if !header_close_button || header_height <= 0.0 {
        return Rect::ZERO;
    }
    Rect::new(inset, (header_height - button_width) / 2.0, button_width, button_width)
}

/// `closeButtonRect` with the production constants (22 pt at inset 6).
pub fn default_close_button_rect(header_close_button: bool, header_height: f64) -> Rect {
    close_button_rect(header_close_button, header_height, CLOSE_BUTTON_SIZE, CLOSE_BUTTON_INSET)
}

/// `iconButtonRect`: x = 32 with a close button, else 6.
pub fn icon_button_rect(header_close_button: bool, header_height: f64) -> Rect {
    let x = if header_close_button { 32.0 } else { 6.0 };
    Rect::new(x, (header_height - ICON_BUTTON_HEIGHT) / 2.0, ICON_BUTTON_WIDTH, ICON_BUTTON_HEIGHT)
}

/// `navRect(_:)`: the capsule track cell for nav icon `index`.
pub fn nav_rect(
    index: usize,
    header_icon: bool,
    header_close_button: bool,
    header_height: f64,
) -> Rect {
    let x0 = if header_icon {
        icon_button_rect(header_close_button, header_height).max_x() + 6.0
    } else if header_close_button {
        default_close_button_rect(true, header_height).max_x() + 6.0
    } else {
        6.0
    };
    Rect::new(
        x0 + 3.0 + index as f64 * (NAV_WIDTH + NAV_GAP),
        (header_height - NAV_HEIGHT) / 2.0,
        NAV_WIDTH,
        NAV_HEIGHT,
    )
}

/// `leftInset`: the x where the header's own content starts.
pub fn left_inset(
    nav_count: usize,
    header_icon: bool,
    header_close_button: bool,
    header_height: f64,
) -> f64 {
    if nav_count > 0 && header_height > 0.0 {
        return nav_rect(nav_count - 1, header_icon, header_close_button, header_height).max_x() + 10.0;
    }
    if header_icon {
        return icon_button_rect(header_close_button, header_height).max_x() + 8.0;
    }
    if header_close_button {
        return default_close_button_rect(true, header_height).max_x() + 8.0;
    }
    10.0
}

/// `headerTitle` handling: the quiet header style carries no title (Figma
/// direction C / `themedRoot` setting `headerTitle = nil`); an empty title is
/// never drawn either.
pub fn effective_header_title(style: HeaderStyle, title: Option<&str>) -> Option<String> {
    match title {
        Some(t) if !t.is_empty() && style != HeaderStyle::Quiet => Some(t.to_string()),
        _ => None,
    }
}

/// The theme tokens `drawHeaderBackground` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderTokens {
    pub background: Rgba,
    pub hairline: Rgba,
    pub accent: Rgba,
    pub accent2: Rgba,
    /// `headerColorOverride ?? config.headerColor ?? config.colors.background`.
    pub header_color: Rgba,
}

impl HeaderTokens {
    pub fn from_colors(colors: &PopupColors, header_color: Option<Rgba>) -> Self {
        HeaderTokens {
            background: colors.background,
            hairline: colors.hairline(),
            accent: colors.accent,
            accent2: colors.palette.accent2,
            header_color: header_color.unwrap_or(colors.background),
        }
    }

    /// `mix(_:_:)`: blend `c` into the opaque base by `t`, then restore base
    /// alpha.
    pub fn mix(&self, c: Rgba, t: f64) -> Rgba {
        let alpha = self.header_color.a;
        self.header_color
            .with_alpha(1.0)
            .blended(t, c.with_alpha(1.0))
            .with_alpha(alpha)
    }
}

/// One paint operation of `drawHeaderBackground`.
#[derive(Clone, Debug, PartialEq)]
pub enum HeaderPaint {
    /// Fill a rect with a solid color.
    Fill(Rect, Rgba),
    /// `bottomLine(_:_:)`: a rule of `width` at the header's bottom edge.
    BottomLine(f64, Rgba),
    /// A gradient in `rect` at `angle`, stops as `(location, color)`.
    Gradient { rect: Rect, stops: Vec<(f64, Rgba)>, angle: f64 },
}

/// `drawHeaderBackground` as a pure op list for every [`HeaderStyle`].
pub fn header_background_ops(
    style: HeaderStyle,
    header: Rect,
    t: &HeaderTokens,
) -> Vec<HeaderPaint> {
    use HeaderPaint::*;
    let base = t.header_color;
    match style {
        HeaderStyle::Quiet => vec![Fill(header, t.background), BottomLine(1.0, t.hairline)],
        HeaderStyle::Flat => vec![Fill(header, base), BottomLine(1.0, t.hairline)],
        HeaderStyle::Edge => {
            vec![Fill(header, base), BottomLine(2.0, t.accent.with_alpha(0.9))]
        }
        HeaderStyle::Stripe => vec![
            Fill(header, base),
            BottomLine(1.0, t.hairline),
            Gradient {
                rect: Rect::new(0.0, 0.0, header.width, 3.0),
                stops: vec![(0.0, t.accent), (1.0, t.accent2)],
                angle: 0.0,
            },
        ],
        HeaderStyle::Tinted => vec![
            Fill(header, t.mix(t.accent, 0.20)),
            BottomLine(1.0, t.accent.with_alpha(0.35)),
        ],
        HeaderStyle::Glow => vec![
            Gradient {
                rect: header,
                stops: vec![(0.0, t.mix(t.accent, 0.40)), (1.0, base)],
                angle: 90.0,
            },
            BottomLine(1.0, t.accent.with_alpha(0.55)),
        ],
        HeaderStyle::Aurora => vec![
            Gradient {
                rect: header,
                stops: vec![
                    (0.0, t.mix(t.accent, 0.36)),
                    (0.45, t.mix(t.accent2, 0.26)),
                    (1.0, base),
                ],
                angle: 0.0,
            },
            BottomLine(1.0, t.accent2.with_alpha(0.35)),
        ],
    }
}

/// A nav icon model (`navIcons` entries); the image is optional so the model
/// stays usable in tests.
#[derive(Clone)]
pub struct NavIcon {
    pub id: i32,
    pub tip: String,
    pub image: Option<Retained<NSImage>>,
}

/// The header config `PopupChrome` needs (a trimmed `PopupConfig`).
#[derive(Clone, Debug)]
pub struct ChromeConfig {
    pub colors: PopupColors,
    pub header_color: Option<Rgba>,
    pub header_height: f64,
    pub title_pill: bool,
    pub header_close_button: bool,
    pub button_radius: f64,
    pub corner_radius: f64,
    pub tint_alpha: f64,
    pub stretch_header_buttons: bool,
}

impl Default for ChromeConfig {
    fn default() -> Self {
        ChromeConfig {
            colors: PopupColors::default(),
            header_color: None,
            header_height: 30.0,
            title_pill: true,
            header_close_button: true,
            button_radius: 4.0,
            corner_radius: 9.0,
            tint_alpha: 0.78,
            stretch_header_buttons: false,
        }
    }
}

#[cfg(target_os = "macos")]
pub use appkit::PopupChrome;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{
        define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSApplication, NSBezierPath, NSColor, NSColorSpace, NSGradient, NSImage, NSLineCapStyle,
        NSView,
    };
    use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn to_nsrect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height))
    }

    fn fill_rect(r: Rect, c: Rgba) {
        c.to_nscolor().setFill();
        NSBezierPath::bezierPathWithRect(to_nsrect(r)).fill();
    }

    fn draw_gradient(rect: Rect, stops: &[(f64, Rgba)], angle: f64) {
        let nsrect = to_nsrect(rect);
        if stops.len() == 2 && stops[0].0 == 0.0 && stops[1].0 == 1.0 {
            let a = stops[0].1.to_nscolor();
            let b = stops[1].1.to_nscolor();
            if let Some(g) = NSGradient::initWithStartingColor_endingColor(NSGradient::alloc(), &a, &b) {
                g.drawInRect_angle(nsrect, angle);
            }
            return;
        }
        let colors: Vec<Retained<NSColor>> = stops.iter().map(|(_, c)| c.to_nscolor()).collect();
        let arr = NSArray::from_retained_slice(&colors);
        let locs: Vec<f64> = stops.iter().map(|(l, _)| *l).collect();
        let space = NSColorSpace::sRGBColorSpace();
        // SAFETY: `locs` outlives the call and `space` is a valid color space.
        unsafe {
            if let Some(g) = NSGradient::initWithColors_atLocations_colorSpace(
                NSGradient::alloc(),
                &arr,
                locs.as_ptr(),
                &space,
            ) {
                g.drawInRect_angle(nsrect, angle);
            }
        }
    }

    /// `PopupChrome` instance variables.
    pub struct PopupChromeIvars {
        pub config: ChromeConfig,
        pub drag_header_height: Cell<f64>,
        pub header_title: RefCell<Option<String>>,
        pub header_icon: RefCell<Option<Retained<NSImage>>>,
        pub copy_path_label: RefCell<String>,
        pub copy_config_label: RefCell<String>,
        pub extra_buttons: RefCell<Vec<(String, i32)>>,
        pub nav_icons: RefCell<Vec<NavIcon>>,
        pub nav_on: Cell<Option<i32>>,
        pub header_color_override: Cell<Option<Rgba>>,
        pub extra_button_rects: RefCell<HashMap<i32, Rect>>,
        pub icon_hovered: Cell<bool>,
        pub close_hovered: Cell<bool>,
        pub icon_menu_open: Cell<bool>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPopupChrome"]
        #[ivars = PopupChromeIvars]
        pub struct PopupChrome;

        impl PopupChrome {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                let h = self.ivars().drag_header_height.get();
                if h <= 0.0 {
                    return;
                }
                let bounds = self.bounds();
                let header = Rect::new(0.0, 0.0, bounds.size.width, h);
                self.draw_header_background(header);
                self.draw_close_glyph();
            }
        }

        unsafe impl NSObjectProtocol for PopupChrome {}
    );

    impl PopupChrome {
        pub fn create(mtm: MainThreadMarker, config: ChromeConfig) -> Retained<PopupChrome> {
            let ivars = PopupChromeIvars {
                drag_header_height: Cell::new(0.0),
                config,
                header_title: RefCell::new(None),
                header_icon: RefCell::new(None),
                copy_path_label: RefCell::new(String::new()),
                copy_config_label: RefCell::new(String::new()),
                extra_buttons: RefCell::new(Vec::new()),
                nav_icons: RefCell::new(Vec::new()),
                nav_on: Cell::new(None),
                header_color_override: Cell::new(None),
                extra_button_rects: RefCell::new(HashMap::new()),
                icon_hovered: Cell::new(false),
                close_hovered: Cell::new(false),
                icon_menu_open: Cell::new(false),
            };
            let this = PopupChrome::alloc(mtm).set_ivars(ivars);
            let view: Retained<PopupChrome> =
                unsafe { msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))] };
            view
        }

        pub fn config(&self) -> ChromeConfig {
            self.ivars().config.clone()
        }

        pub fn set_drag_header_height(&self, h: f64) {
            self.ivars().drag_header_height.set(h);
            self.setNeedsDisplay(true);
        }

        pub fn drag_header_height(&self) -> f64 {
            self.ivars().drag_header_height.get()
        }

        pub fn set_header_title(&self, title: Option<String>) {
            *self.ivars().header_title.borrow_mut() = title;
            self.setNeedsDisplay(true);
        }

        pub fn header_title(&self) -> Option<String> {
            self.ivars().header_title.borrow().clone()
        }

        pub fn set_header_icon(&self, icon: Option<Retained<NSImage>>) {
            *self.ivars().header_icon.borrow_mut() = icon;
            self.setNeedsDisplay(true);
        }

        pub fn header_icon(&self) -> Option<Retained<NSImage>> {
            self.ivars().header_icon.borrow().clone()
        }

        pub fn set_copy_labels(&self, path: &str, config: &str) {
            *self.ivars().copy_path_label.borrow_mut() = path.to_string();
            *self.ivars().copy_config_label.borrow_mut() = config.to_string();
            self.setNeedsDisplay(true);
        }

        pub fn set_extra_buttons(&self, buttons: Vec<(String, i32)>) {
            *self.ivars().extra_buttons.borrow_mut() = buttons;
            self.setNeedsDisplay(true);
        }

        pub fn set_nav_icons(&self, icons: Vec<NavIcon>) {
            *self.ivars().nav_icons.borrow_mut() = icons;
            self.setNeedsDisplay(true);
        }

        pub fn set_nav_on(&self, on: Option<i32>) {
            self.ivars().nav_on.set(on);
            self.setNeedsDisplay(true);
        }

        pub fn set_header_color_override(&self, color: Option<Rgba>) {
            self.ivars().header_color_override.set(color);
            self.setNeedsDisplay(true);
        }

        pub fn header_color(&self) -> Rgba {
            self.ivars()
                .header_color_override
                .get()
                .or(self.ivars().config.header_color)
                .unwrap_or(self.ivars().config.colors.background)
        }

        pub fn close_button_rect(&self) -> Rect {
            default_close_button_rect(
                self.ivars().config.header_close_button,
                self.ivars().drag_header_height.get(),
            )
        }

        pub fn icon_button_rect(&self) -> Rect {
            icon_button_rect(
                self.ivars().config.header_close_button,
                self.ivars().drag_header_height.get(),
            )
        }

        pub fn nav_rect(&self, index: usize) -> Rect {
            nav_rect(
                index,
                self.ivars().header_icon.borrow().is_some(),
                self.ivars().config.header_close_button,
                self.ivars().drag_header_height.get(),
            )
        }

        pub fn left_inset(&self) -> f64 {
            left_inset(
                self.ivars().nav_icons.borrow().len(),
                self.ivars().header_icon.borrow().is_some(),
                self.ivars().config.header_close_button,
                self.ivars().drag_header_height.get(),
            )
        }

        pub fn extra_button_rects(&self) -> HashMap<i32, Rect> {
            self.ivars().extra_button_rects.borrow().clone()
        }

        pub fn set_icon_hovered(&self, on: bool) {
            self.ivars().icon_hovered.set(on);
            self.setNeedsDisplay(true);
        }

        pub fn set_close_hovered(&self, on: bool) {
            self.ivars().close_hovered.set(on);
            self.setNeedsDisplay(true);
        }

        pub fn set_icon_menu_open(&self, on: bool) {
            self.ivars().icon_menu_open.set(on);
            self.setNeedsDisplay(true);
        }

        /// `drawHeaderBackground(_:)`: paint ops for the current global style.
        pub fn draw_header_background(&self, header: Rect) {
            let tokens = HeaderTokens::from_colors(&self.ivars().config.colors, Some(self.header_color()));
            for op in header_background_ops(current_header_style(), header, &tokens) {
                match op {
                    HeaderPaint::Fill(r, c) => fill_rect(r, c),
                    HeaderPaint::BottomLine(w, c) => {
                        fill_rect(Rect::new(header.x, header.max_y() - w, header.width, w), c);
                    }
                    HeaderPaint::Gradient { rect, stops, angle } => {
                        draw_gradient(rect, &stops, angle);
                    }
                }
            }
        }

        /// `drawCloseGlyph`: the hover dot + the ✕ strokes.
        fn draw_close_glyph(&self) {
            let close = self.close_button_rect();
            if close.is_empty() {
                return;
            }
            let colors = &self.ivars().config.colors;
            let dot = close.inset_by(3.0, 3.0);
            let x_color = if self.ivars().close_hovered.get() {
                let danger = colors.tone(PopupTone::Danger);
                danger.to_nscolor().setFill();
                NSBezierPath::bezierPathWithOvalInRect(to_nsrect(dot)).fill();
                colors.readable(colors.crust(), 3.0)
            } else {
                colors.dim
            };
            let r = 3.2;
            let x = NSBezierPath::bezierPath();
            x.moveToPoint(NSPoint::new(dot.mid_x() - r, dot.mid_y() - r));
            x.lineToPoint(NSPoint::new(dot.mid_x() + r, dot.mid_y() + r));
            x.moveToPoint(NSPoint::new(dot.mid_x() + r, dot.mid_y() - r));
            x.lineToPoint(NSPoint::new(dot.mid_x() - r, dot.mid_y() + r));
            x.setLineWidth(1.6);
            x.setLineCapStyle(NSLineCapStyle::Round);
            x_color.to_nscolor().setStroke();
            x.stroke();
        }

        /// `PopupChrome.redrawAll()`.
        pub fn redraw_all(mtm: MainThreadMarker) {
            let app = NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in &windows {
                if let Some(v) = w.contentView() {
                    Self::walk_redraw(&v);
                }
            }
        }

        fn walk_redraw(view: &NSView) {
            if let Some(ch) = as_any(view).downcast_ref::<PopupChrome>() {
                ch.setNeedsDisplay(true);
            }
            let subs = view.subviews();
            for s in &subs {
                Self::walk_redraw(&s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_button_rect_zero_when_disabled_or_collapsed() {
        assert_eq!(close_button_rect(false, 30.0, 22.0, 6.0), Rect::ZERO);
        assert_eq!(close_button_rect(true, 0.0, 22.0, 6.0), Rect::ZERO);
        assert_eq!(close_button_rect(true, -5.0, 22.0, 6.0), Rect::ZERO);
    }

    #[test]
    fn close_button_rect_centered_at_inset() {
        let r = close_button_rect(true, 30.0, 22.0, 6.0);
        assert_eq!(r, Rect::new(6.0, 4.0, 22.0, 22.0));
        assert_eq!(r.mid_y(), 15.0);
        assert_eq!(r.max_x(), 28.0);
        // Production constants match.
        assert_eq!(default_close_button_rect(true, 30.0), r);
    }

    #[test]
    fn icon_button_shifts_with_close_button() {
        let with_close = icon_button_rect(true, 30.0);
        assert_eq!(with_close, Rect::new(32.0, 4.0, 40.0, 22.0));
        let without = icon_button_rect(false, 30.0);
        assert_eq!(without, Rect::new(6.0, 4.0, 40.0, 22.0));
    }

    #[test]
    fn nav_rect_walks_by_width_plus_gap() {
        let r0 = nav_rect(0, false, true, 30.0);
        // x0 = close maxX(28) + 6 = 34, +3 = 37
        assert_eq!(r0, Rect::new(37.0, 3.0, 32.0, 24.0));
        let r1 = nav_rect(1, false, true, 30.0);
        assert_eq!(r1.x, r0.x + NAV_WIDTH + NAV_GAP);
        // With an icon button, x0 = icon maxX(72) + 6 = 78.
        let r_icon = nav_rect(0, true, true, 30.0);
        assert_eq!(r_icon.x, 78.0 + 3.0);
    }

    #[test]
    fn left_inset_prefers_nav_then_icon_then_close() {
        assert_eq!(left_inset(0, false, false, 30.0), 10.0);
        assert_eq!(left_inset(0, false, true, 30.0), 28.0 + 8.0);
        assert_eq!(left_inset(0, true, true, 30.0), 72.0 + 8.0);
        let nav = left_inset(2, false, true, 30.0);
        assert_eq!(nav, nav_rect(1, false, true, 30.0).max_x() + 10.0);
    }

    #[test]
    fn quiet_header_has_no_title() {
        assert_eq!(effective_header_title(HeaderStyle::Quiet, Some("Jira")), None);
        assert_eq!(
            effective_header_title(HeaderStyle::Flat, Some("Jira")),
            Some("Jira".to_string())
        );
        assert_eq!(effective_header_title(HeaderStyle::Flat, Some("")), None);
        assert_eq!(effective_header_title(HeaderStyle::Flat, None), None);
    }

    #[test]
    fn quiet_and_flat_paint_ops() {
        let colors = PopupColors::default();
        let t = HeaderTokens::from_colors(&colors, None);
        let header = Rect::new(0.0, 0.0, 300.0, 30.0);

        let quiet = header_background_ops(HeaderStyle::Quiet, header, &t);
        assert_eq!(
            quiet,
            vec![
                HeaderPaint::Fill(header, colors.background),
                HeaderPaint::BottomLine(1.0, colors.hairline()),
            ]
        );

        let flat = header_background_ops(HeaderStyle::Flat, header, &t);
        assert_eq!(flat[0], HeaderPaint::Fill(header, colors.background));
        assert_eq!(flat[1], HeaderPaint::BottomLine(1.0, colors.hairline()));
    }

    #[test]
    fn every_style_has_a_bottom_rule() {
        let colors = PopupColors::default();
        let t = HeaderTokens::from_colors(&colors, Some(Rgba::from_u8(10, 20, 30, 200)));
        let header = Rect::new(0.0, 0.0, 300.0, 30.0);
        for style in HeaderStyle::ALL {
            let ops = header_background_ops(style, header, &t);
            assert!(!ops.is_empty(), "{style:?}");
            assert!(
                ops.iter().any(|o| matches!(o, HeaderPaint::BottomLine(..))),
                "{style:?} must carry a bottom rule"
            );
        }
    }

    #[test]
    fn mix_restores_base_alpha() {
        let colors = PopupColors::default();
        let t = HeaderTokens::from_colors(&colors, Some(Rgba::new(0.1, 0.1, 0.1, 0.5)));
        let m = t.mix(colors.accent, 0.2);
        assert!((m.a - 0.5).abs() < 1e-9);
        let expected = t
            .header_color
            .with_alpha(1.0)
            .blended(0.2, colors.accent.with_alpha(1.0))
            .with_alpha(0.5);
        assert_eq!(m, expected);
    }

    #[test]
    fn stripe_carries_a_top_gradient() {
        let colors = PopupColors::default();
        let t = HeaderTokens::from_colors(&colors, None);
        let header = Rect::new(0.0, 0.0, 300.0, 30.0);
        let ops = header_background_ops(HeaderStyle::Stripe, header, &t);
        let g = ops
            .iter()
            .find_map(|o| match o {
                HeaderPaint::Gradient { rect, stops, angle } => Some((*rect, stops.clone(), *angle)),
                _ => None,
            })
            .expect("stripe gradient");
        assert_eq!(g.0, Rect::new(0.0, 0.0, 300.0, 3.0));
        assert_eq!(g.1, vec![(0.0, colors.accent), (1.0, colors.palette.accent2)]);
        assert_eq!(g.2, 0.0);
    }
}
