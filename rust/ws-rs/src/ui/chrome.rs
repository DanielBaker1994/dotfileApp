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

/// `PopupChrome.workspaceBase` — header ids at/above this are workspace cells.
pub const WORKSPACE_BASE: i32 = 1000;
/// Header click id for the ✕ glyph.
pub const HEADER_CLOSE_ID: i32 = -1;
/// Header click id for the app-icon menu button.
pub const HEADER_ICON_ID: i32 = -2;
/// `wsCellH`.
pub const WS_CELL_HEIGHT: f64 = 24.0;

/// One workspace strip cell's width (`wsCellRects` widths): key + icons
/// (+ "+N" extra) (+ unread badge).
pub fn ws_cell_width(icons: usize, extra: bool, unread: bool) -> f64 {
    let mut w = 10.0 + 9.0 + 6.0 + icons as f64 * 19.0 + 4.0;
    if extra {
        w += 18.0;
    }
    if unread {
        w += 22.0;
    }
    w
}

/// `wsCellRects()`: right-aligned cells ending at `ws_right - 3`, dropping
/// leading cells until the strip clears `min_x`. Dropped cells are `None`.
pub fn ws_cell_rects(widths: &[f64], min_x: f64, ws_right: f64, header_h: f64) -> Vec<Option<Rect>> {
    if widths.is_empty() || header_h <= 0.0 || ws_right <= 0.0 {
        return Vec::new();
    }
    let total = |f: usize| -> f64 {
        widths[f..].iter().sum::<f64>() + (widths.len() - f).saturating_sub(1) as f64 * 2.0
    };
    let mut first = 0;
    while first < widths.len() && ws_right - 3.0 - total(first) < min_x {
        first += 1;
    }
    if first >= widths.len() {
        return Vec::new();
    }
    let mut x = ws_right - 3.0 - total(first);
    let y = (header_h - WS_CELL_HEIGHT) / 2.0;
    let mut out: Vec<Option<Rect>> = vec![None; first];
    for w in &widths[first..] {
        out.push(Some(Rect::new(x, y, *w, WS_CELL_HEIGHT)));
        x += w + 2.0;
    }
    out
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
pub use appkit::{set_workspace_cells, workspace_cell_key, PopupChrome, WorkspaceCell};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{
        define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSApplication, NSBezierPath, NSColor, NSColorSpace, NSEvent, NSGradient, NSImage,
        NSLineCapStyle, NSTrackingArea, NSTrackingAreaOptions, NSView,
    };
    use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use crate::ui::style;

    /// `PopupChrome.WorkspaceCell`.
    #[derive(Clone)]
    pub struct WorkspaceCell {
        pub key: String,
        pub icons: Vec<Retained<NSImage>>,
        pub extra: Option<String>,
        pub unread: String,
        pub focused: bool,
    }

    thread_local! {
        /// `PopupChrome.workspaceCells` (static, shared by every chrome).
        static WORKSPACE_CELLS: RefCell<Vec<WorkspaceCell>> = const { RefCell::new(Vec::new()) };
    }

    /// Replace the strip cells and redraw every chrome (main thread).
    pub fn set_workspace_cells(mtm: MainThreadMarker, cells: Vec<WorkspaceCell>) {
        WORKSPACE_CELLS.with(|c| *c.borrow_mut() = cells);
        PopupChrome::redraw_all(mtm);
    }

    pub fn workspace_cell_key(index: usize) -> Option<String> {
        WORKSPACE_CELLS.with(|c| c.borrow().get(index).map(|w| w.key.clone()))
    }

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
        pub hovered: Cell<Option<i32>>,
        pub on_click: RefCell<Option<Box<dyn Fn(i32)>>>,
        pub tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        pub tip_owners: RefCell<Vec<Retained<NSString>>>,
        pub tip_rects: RefCell<Vec<Rect>>,
        /// `itemCount` / `footerText` — the dim hint after the left inset.
        pub meta: RefCell<String>,
        /// Force-draw the title even in the quiet style (tool panels).
        pub always_title: Cell<bool>,
        /// The ✓ feedback segment id (`feedback`) and when it expires.
        pub feedback: Cell<i32>,
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
                self.draw_title(bounds.size.width);
                self.draw_close_glyph();
                if let Some(icon) = self.ivars().header_icon.borrow().clone() {
                    self.draw_icon_menu_button(&icon);
                }
                self.ivars().extra_button_rects.borrow_mut().clear();
                let bar_w = self.draw_header_segs(bounds.size.width);
                if !self.ivars().nav_icons.borrow().is_empty() {
                    self.draw_nav_icons();
                    self.draw_workspace_cells(bounds.size.width - 10.0 - bar_w);
                }
                self.draw_meta(bounds.size.width - bar_w - 20.0);
            }

            #[unsafe(method(updateTrackingAreas))]
            fn update_tracking_areas(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                let opts = NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect;
                let t = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        NSTrackingArea::alloc(),
                        self.bounds(),
                        opts,
                        Some(as_any(self)),
                        None,
                    )
                };
                self.addTrackingArea(&t);
                *self.ivars().tracking.borrow_mut() = Some(t);
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, event: &NSEvent) {
                let p = self.convertPoint_fromView(event.locationInWindow(), None);
                let hit = self.hit_id(p.x, p.y);
                self.ivars().close_hovered.set(hit == Some(HEADER_CLOSE_ID));
                self.ivars().icon_hovered.set(hit == Some(HEADER_ICON_ID));
                if hit != self.ivars().hovered.get() {
                    self.ivars().hovered.set(hit);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _event: &NSEvent) {
                self.ivars().close_hovered.set(false);
                self.ivars().icon_hovered.set(false);
                self.ivars().hovered.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                let p = self.convertPoint_fromView(event.locationInWindow(), None);
                if let Some(id) = self.hit_id(p.x, p.y) {
                    if let Some(cb) = self.ivars().on_click.borrow().as_ref() {
                        cb(id);
                    }
                    return;
                }
                if let Some(w) = self.window() {
                    w.performWindowDragWithEvent(event);
                }
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
                hovered: Cell::new(None),
                on_click: RefCell::new(None),
                tracking: RefCell::new(None),
                tip_owners: RefCell::new(Vec::new()),
                tip_rects: RefCell::new(Vec::new()),
                meta: RefCell::new(String::new()),
                always_title: Cell::new(false),
                feedback: Cell::new(0),
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

        /// Route header clicks (nav ids, `WORKSPACE_BASE + i`, the close /
        /// icon ids) to `cb` (Swift `onHeaderButton`).
        pub fn set_on_click(&self, cb: Box<dyn Fn(i32)>) {
            *self.ivars().on_click.borrow_mut() = Some(cb);
        }

        /// The header id under (x, y), if any.
        pub fn hit_id(&self, x: f64, y: f64) -> Option<i32> {
            let h = self.ivars().drag_header_height.get();
            if h <= 0.0 || y > h {
                return None;
            }
            if self.close_button_rect().contains(x, y) {
                return Some(HEADER_CLOSE_ID);
            }
            if self.ivars().header_icon.borrow().is_some() && self.icon_button_rect().contains(x, y) {
                return Some(HEADER_ICON_ID);
            }
            for (id, r) in self.seg_rects(self.bounds().size.width) {
                if r.contains(x, y) {
                    return Some(id);
                }
            }
            let icons = self.ivars().nav_icons.borrow();
            for (i, n) in icons.iter().enumerate() {
                if self.nav_rect(i).contains(x, y) {
                    return Some(n.id);
                }
            }
            if !icons.is_empty() {
                let w = self.bounds().size.width - 10.0;
                for (i, r) in self.ws_rects(w).iter().enumerate() {
                    if let Some(r) = r {
                        if r.contains(x, y) {
                            return Some(WORKSPACE_BASE + i as i32);
                        }
                    }
                }
            }
            None
        }

        fn ws_rects(&self, ws_right: f64) -> Vec<Option<Rect>> {
            let n = self.ivars().nav_icons.borrow().len();
            if n == 0 {
                return Vec::new();
            }
            let widths: Vec<f64> = WORKSPACE_CELLS.with(|c| {
                c.borrow()
                    .iter()
                    .map(|w| ws_cell_width(w.icons.len(), w.extra.is_some(), !w.unread.is_empty()))
                    .collect()
            });
            let min_x = self.nav_rect(n - 1).max_x() + 24.0;
            ws_cell_rects(&widths, min_x, ws_right, self.ivars().drag_header_height.get())
        }

        /// Show the dim header hint (`footerText`).
        pub fn set_meta(&self, text: &str) {
            *self.ivars().meta.borrow_mut() = text.to_string();
            self.setNeedsDisplay(true);
        }

        pub fn set_always_title(&self, on: bool) {
            self.ivars().always_title.set(on);
            self.setNeedsDisplay(true);
        }

        /// Flash "✓ " on segment `id` (`flashFeedback`) for ~1.2 s.
        pub fn flash(&self, id: i32) {
            self.ivars().feedback.set(id);
            self.setNeedsDisplay(true);
            let Some(mtm) = MainThreadMarker::new() else { return };
            let bound = dispatch2::MainThreadBound::new(objc2::rc::Weak::from_retained(&self.retain()), mtm);
            dispatch2::DispatchQueue::main().after(
                dispatch2::DispatchTime::NOW.time(1_200_000_000),
                move || {
                    let Some(mtm) = MainThreadMarker::new() else { return };
                    if let Some(ch) = bound.get(mtm).load() {
                        if ch.ivars().feedback.get() == id {
                            ch.ivars().feedback.set(0);
                            ch.setNeedsDisplay(true);
                        }
                    }
                },
            ).ok();
        }

        /// `headerSegs()` — copy path (1), copy config (2), extras (≥ 10).
        fn segs(&self) -> Vec<(String, i32)> {
            let mut out = Vec::new();
            let p = self.ivars().copy_path_label.borrow();
            if !p.is_empty() {
                out.push((p.clone(), 1));
            }
            let c = self.ivars().copy_config_label.borrow();
            if !c.is_empty() {
                out.push((c.clone(), 2));
            }
            out.extend(self.ivars().extra_buttons.borrow().iter().cloned());
            out
        }

        /// The right-aligned segmented bar rects.
        fn seg_rects(&self, width: f64) -> Vec<(i32, Rect)> {
            let h = self.ivars().drag_header_height.get();
            let segs = self.segs();
            if segs.is_empty() || h <= 0.0 {
                return Vec::new();
            }
            let f = style::system_font(10.5, style::Weight::Semibold);
            let fb = self.ivars().feedback.get();
            let ws: Vec<f64> = segs
                .iter()
                .map(|(t, id)| {
                    let t = if *id == fb { format!("✓ {t}") } else { t.clone() };
                    style::text_size(&t, &f).0 + 30.0
                })
                .collect();
            let total: f64 = ws.iter().sum::<f64>() + (ws.len() - 1) as f64;
            let mut x = width - 10.0 - total;
            let y = (h - 20.0) / 2.0;
            segs.iter()
                .zip(ws)
                .map(|((_, id), w)| {
                    let r = Rect::new(x, y, w, 20.0);
                    x += w + 1.0;
                    (*id, r)
                })
                .collect()
        }

        /// Draw the header buttons (`ButtonStyle` bar); returns its width.
        fn draw_header_segs(&self, width: f64) -> f64 {
            let rects = self.seg_rects(width);
            if rects.is_empty() {
                return 0.0;
            }
            let c = self.ivars().config.colors;
            let segs = self.segs();
            let bar = Rect::new(rects[0].1.x, rects[0].1.y, rects.last().unwrap().1.max_x() - rects[0].1.x, 20.0);
            style::draw_button(bar, style::ButtonState::Idle, &c, self.ivars().config.button_radius, false);
            let f = style::system_font(10.5, style::Weight::Semibold);
            let hover = self.ivars().hovered.get();
            let fb = self.ivars().feedback.get();
            for (i, ((label, id), (_, r))) in segs.iter().zip(rects.iter()).enumerate() {
                let st = if *id == fb {
                    style::ButtonState::Pressed
                } else if hover == Some(*id) {
                    style::ButtonState::Hover
                } else {
                    style::ButtonState::Idle
                };
                if i > 0 && st == style::ButtonState::Idle && hover != Some(segs[i - 1].1) {
                    style::vline(r.x, r.y + 5.0, r.max_y() - 5.0, c.text.with_alpha(0.10));
                }
                if st != style::ButtonState::Idle {
                    style::draw_button(r.inset_by(2.0, 2.0), st, &c, self.ivars().config.button_radius - 2.0, false);
                }
                let t = if *id == fb { format!("✓ {label}") } else { label.clone() };
                style::draw_text_centered(&t, *r, &f, style::button_text(st, &c));
                self.ivars().extra_button_rects.borrow_mut().insert(*id, *r);
            }
            bar.width + 10.0
        }

        /// `meta` — the dim hint after the left inset (truncated).
        fn draw_meta(&self, right: f64) {
            let meta = self.ivars().meta.borrow().clone();
            if meta.is_empty() {
                return;
            }
            let c = self.ivars().config.colors;
            let h = self.ivars().drag_header_height.get();
            let x0 = self.left_inset();
            let f = style::system_font(10.0, style::Weight::Regular);
            style::draw_text_left(&meta, Rect::new(x0, 0.0, (right - x0).max(60.0), h), &f, c.dim);
        }

        /// The header title (non-quiet styles), centered like Swift's pill.
        fn draw_title(&self, width: f64) {
            let title = self.ivars().header_title.borrow().clone();
            let title = if self.ivars().always_title.get() {
                title.filter(|t| !t.is_empty())
            } else {
                effective_header_title(current_header_style(), title.as_deref())
            };
            let Some(title) = title else {
                return;
            };
            let c = &self.ivars().config.colors;
            let font = style::system_font(13.5, style::Weight::Bold);
            let (tw, th) = style::text_size(&title, &font);
            let pad_x = 12.0;
            let center = (tw / 2.0 + pad_x).max((width - 10.0) / 2.0);
            let pill = Rect::new(center - tw / 2.0 - pad_x, 4.0, tw + pad_x * 2.0, th + 8.0);
            if self.ivars().config.title_pill {
                style::fill_rounded(pill, 7.0, c.highlight.with_alpha(0.35));
            }
            style::draw_text(&title, pill.mid_x() - tw / 2.0, pill.mid_y() - th / 2.0, &font, c.text);
        }

        /// `drawIconMenuButton(_:)` — the app icon + a chevron.
        fn draw_icon_menu_button(&self, icon: &NSImage) {
            let c = &self.ivars().config.colors;
            let badge = self.icon_button_rect();
            let st = if self.ivars().icon_menu_open.get() {
                style::ButtonState::On
            } else if self.ivars().icon_hovered.get() {
                style::ButtonState::Hover
            } else {
                style::ButtonState::Idle
            };
            style::draw_button(badge, st, c, self.ivars().config.button_radius, true);
            let isz = 16.0;
            style::draw_image(icon, Rect::new(badge.x + 6.0, badge.mid_y() - isz / 2.0, isz, isz), 1.0);
            style::chevron(
                Rect::new(badge.max_x() - 13.0, badge.mid_y() - 4.0, 8.0, 8.0),
                style::button_text(st, c),
            );
        }

        /// `drawNavIcons()` — one capsule track, a chip per view, the icon.
        fn draw_nav_icons(&self) {
            let c = self.ivars().config.colors;
            let icons = self.ivars().nav_icons.borrow();
            let first = self.nav_rect(0);
            let last = self.nav_rect(icons.len() - 1);
            let well = Rect::new(
                first.x - 3.0,
                first.y - 3.0,
                last.max_x() - first.x + 6.0,
                first.height + 6.0,
            );
            style::capsule_track(well, &c);
            let on = self.ivars().nav_on.get();
            let hovered = self.ivars().hovered.get();
            let mut tips: Vec<Rect> = Vec::new();
            for (i, n) in icons.iter().enumerate() {
                let r = self.nav_rect(i);
                let is_on = on == Some(n.id);
                let hov = hovered == Some(n.id);
                style::capsule_chip(r, &c, is_on, hov);
                let isz = 16.0;
                if let Some(img) = &n.image {
                    style::draw_image(
                        img,
                        Rect::new(r.mid_x() - isz / 2.0, r.mid_y() - isz / 2.0, isz, isz),
                        if is_on || hov { 1.0 } else { 0.7 },
                    );
                }
                self.ivars().extra_button_rects.borrow_mut().insert(n.id, r);
                tips.push(r);
            }
            if *self.ivars().tip_rects.borrow() != tips {
                self.removeAllToolTips();
                let owners: Vec<Retained<NSString>> =
                    icons.iter().map(|n| NSString::from_str(&n.tip)).collect();
                for (r, o) in tips.iter().zip(owners.iter()) {
                    unsafe {
                        self.addToolTipRect_owner_userData(
                            style::ns(*r),
                            as_any(&**o),
                            std::ptr::null_mut(),
                        );
                    }
                }
                *self.ivars().tip_owners.borrow_mut() = owners;
                *self.ivars().tip_rects.borrow_mut() = tips;
            }
        }

        /// `drawWorkspaceCells()` — the AeroSpace strip at the header's right.
        fn draw_workspace_cells(&self, ws_right: f64) {
            let c = self.ivars().config.colors;
            let rects = self.ws_rects(ws_right);
            let Some(first_shown) = rects.iter().position(|r| r.is_some()) else {
                return;
            };
            let (Some(first), Some(Some(last))) = (rects[first_shown], rects.last()) else {
                return;
            };
            style::capsule_track(
                Rect::new(
                    first.x - 3.0,
                    first.y - 3.0,
                    last.max_x() - first.x + 6.0,
                    first.height + 6.0,
                ),
                &c,
            );
            let hovered = self.ivars().hovered.get();
            let key_font = style::system_font(11.0, style::Weight::Bold);
            let extra_font = style::system_font(10.0, style::Weight::Regular);
            let badge_font = style::system_font(9.5, style::Weight::Semibold);
            WORKSPACE_CELLS.with(|cells| {
                for (i, cell) in cells.borrow().iter().enumerate() {
                    let Some(Some(r)) = rects.get(i).copied() else {
                        continue;
                    };
                    let id = WORKSPACE_BASE + i as i32;
                    let hov = hovered == Some(id);
                    style::capsule_chip(r, &c, cell.focused, hov);
                    let (_, kh) = style::text_size(&cell.key, &key_font);
                    style::draw_text(
                        &cell.key,
                        r.x + 10.0,
                        r.mid_y() - kh / 2.0,
                        &key_font,
                        if cell.focused { c.text } else { c.dim },
                    );
                    let mut x = r.x + 10.0 + 9.0 + 6.0;
                    for img in &cell.icons {
                        style::draw_image(
                            img,
                            Rect::new(x, r.mid_y() - 8.0, 16.0, 16.0),
                            if cell.focused || hov { 1.0 } else { 0.8 },
                        );
                        x += 19.0;
                    }
                    if let Some(extra) = &cell.extra {
                        let (_, eh) = style::text_size(extra, &extra_font);
                        style::draw_text(extra, x, r.mid_y() - eh / 2.0, &extra_font, c.dim);
                        x += 18.0;
                    }
                    if !cell.unread.is_empty() {
                        let (bw0, bh) = style::text_size(&cell.unread, &badge_font);
                        let bw = (bw0 + 9.0).max(15.0);
                        let br = Rect::new(x, r.mid_y() - 7.5, bw, 15.0);
                        style::fill_rounded(br, 7.5, c.tone(PopupTone::Danger));
                        style::draw_text(
                            &cell.unread,
                            br.mid_x() - bw0 / 2.0,
                            br.mid_y() - bh / 2.0,
                            &badge_font,
                            Rgba::new(1.0, 1.0, 1.0, 1.0),
                        );
                    }
                    self.ivars().extra_button_rects.borrow_mut().insert(id, r);
                }
            });
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
