//! Port of `PaneNav.swift` — the pure half: directional pane focus over a
//! window's pane rectangles, focus tracking, and the socket `pane` test state.
//!
//! AppKit (`NSView`, `NSResponder`, window conversions) is out of scope for
//! the model: a pane carries an opaque [`ViewId`] and its already-resolved
//! window-space [`Rect`] (the Swift `part()` / `view.bounds` conversion).
//! [`PaneFocusRing`] (the hairline border) is the `#[cfg(target_os = "macos")]`
//! `NSView` subclass; its style/geometry math is pure and tested here.

use serde::Serialize;

use crate::panes::pane_geometry::{next, remember, Came, PaneDir, PaneRect, Rect};
use crate::ui::theme::{HexColor, Rgba};

/// Opaque identity of an `NSView` / `NSResponder` (AppKit handles have no
/// place in the pure model).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewId(pub u64);

/// `NavPane`: one navigable area. `rect` is the pane's window-space rectangle;
/// `visible` mirrors `NavPane.visible(in:)`; `owns`/`intercept`/`focus` mirror
/// the Swift closures.
pub struct NavPane {
    pub id: String,
    pub view: ViewId,
    pub rect: Rect,
    pub visible: bool,
    /// `owns`: is this responder inside the pane? Defaults to `view == responder`.
    pub owns: Option<Box<dyn Fn(ViewId) -> bool>>,
    /// `intercept`: consume a direction inside the pane (nvim's own splits).
    pub intercept: Option<Box<dyn Fn(PaneDir) -> bool>>,
    /// `focus`: make the pane first responder (AppKit callback).
    pub focus: Option<Box<dyn Fn()>>,
}

impl NavPane {
    pub fn new(id: impl Into<String>, view: ViewId, rect: Rect) -> NavPane {
        NavPane {
            id: id.into(),
            view,
            rect,
            visible: true,
            owns: None,
            intercept: None,
            focus: None,
        }
    }

    pub fn with_owns(mut self, f: impl Fn(ViewId) -> bool + 'static) -> Self {
        self.owns = Some(Box::new(f));
        self
    }

    pub fn with_intercept(mut self, f: impl Fn(PaneDir) -> bool + 'static) -> Self {
        self.intercept = Some(Box::new(f));
        self
    }

    pub fn with_focus(mut self, f: impl Fn() + 'static) -> Self {
        self.focus = Some(Box::new(f));
        self
    }

    /// `NavPane.contains(_:)` with the descendant/field-editor cases folded
    /// into the caller's `owns` closure (only identity is modelable purely).
    pub fn contains(&self, responder: ViewId) -> bool {
        match &self.owns {
            Some(f) => f(responder),
            None => self.view == responder,
        }
    }
}

impl std::fmt::Debug for NavPane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavPane")
            .field("id", &self.id)
            .field("view", &self.view)
            .field("rect", &self.rect)
            .field("visible", &self.visible)
            .finish()
    }
}

/// `PaneProvider`: a view that exposes its navigable panes to [`PaneNav`].
pub trait PaneProvider {
    fn nav_panes(&self) -> Vec<NavPane>;
    /// `paneFocusMoved()` — the keyboard left the previously focused pane.
    fn pane_focus_moved(&self) {}
}

fn area(p: &NavPane) -> f64 {
    p.rect.width * p.rect.height
}

fn box_rect(r: Rect) -> [i64; 4] {
    [r.min_x() as i64, r.min_y() as i64, r.width as i64, r.height as i64]
}

/// One `panes[]` entry of the socket state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PaneEntry {
    pub id: String,
    pub rect: [i64; 4],
}

/// The socket `ring` entry; `rect` is `None` when no ring is shown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RingEntry {
    pub pane: String,
    pub rect: Option<[i64; 4]>,
}

/// The socket `pane` state (`focused`, `panes[]`, `ring`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PaneTestState {
    pub focused: String,
    pub panes: Vec<PaneEntry>,
    pub ring: RingEntry,
}

/// `PaneNav`: focus tracking + directional movement.
pub struct PaneNav {
    /// The current first responder (the pane that `owns` it is "current").
    pub first_responder: Option<ViewId>,
    /// `came`: per-source/direction retrace memory (see `pane_geometry`).
    pub came: Came,
    /// Mirror of `ringState`, kept for `testState`.
    pub ring: Option<RingEntry>,
    /// Whether the window is key (the ring only shows on a key window).
    pub window_key: bool,
    /// Test hook: how many times focus moved (`paneFocusMoved` calls).
    pub focus_moved: u32,
}

impl Default for PaneNav {
    fn default() -> Self {
        PaneNav::new()
    }
}

impl PaneNav {
    pub fn new() -> PaneNav {
        PaneNav {
            first_responder: None,
            came: Came::new(),
            ring: None,
            window_key: true,
            focus_moved: 0,
        }
    }

    /// `panes(in:)`: the visible panes, in order.
    pub fn panes<'a>(&self, list: &'a [NavPane]) -> Vec<&'a NavPane> {
        list.iter().filter(|p| p.visible).collect()
    }

    /// `current(in:)`: the visible pane owning the first responder, smallest
    /// area wins when several match.
    pub fn current<'a>(&self, list: &'a [NavPane]) -> Option<&'a NavPane> {
        let fr = self.first_responder?;
        self.panes(list)
            .into_iter()
            .filter(|p| p.contains(fr))
            .min_by(|a, b| area(a).partial_cmp(&area(b)).unwrap())
    }

    fn take_focus(&mut self, pane: &NavPane) {
        if let Some(f) = &pane.focus {
            f();
        }
        self.first_responder = Some(pane.view);
    }

    /// `move(_:in:)`: move focus one pane in `dir`. Returns whether the key was
    /// used (always true once a provider/list exists).
    pub fn move_dir(&mut self, dir: PaneDir, list: &[NavPane]) -> bool {
        let visible = self.panes(list);
        if visible.is_empty() {
            return false;
        }
        let cur_id = match self.current(list) {
            Some(c) => c.id.clone(),
            None => {
                let biggest = visible
                    .iter()
                    .max_by(|a, b| area(a).partial_cmp(&area(b)).unwrap())
                    .unwrap();
                self.take_focus(biggest);
                self.refresh_state(list);
                return true;
            }
        };
        let cur = visible.iter().find(|p| p.id == cur_id).unwrap();
        if cur.intercept.as_ref().map(|f| f(dir)).unwrap_or(false) {
            return true;
        }
        let rects: Vec<PaneRect> = visible.iter().map(|p| PaneRect::new(&p.id, p.rect)).collect();
        if let Some(to) = next(&cur_id, dir, &rects, Some(&self.came)) {
            if let Some(target) = visible.iter().find(|p| p.id == to) {
                remember(&mut self.came, &cur_id, &to, dir);
                self.take_focus(target);
                self.focus_moved += 1;
            }
        }
        self.refresh_state(list);
        true
    }

    /// `move(_:in:)` through a [`PaneProvider`], firing `paneFocusMoved()`.
    pub fn move_provider(&mut self, dir: PaneDir, provider: &dyn PaneProvider) -> bool {
        let before = self.focus_moved;
        let moved = self.move_dir(dir, &provider.nav_panes());
        if self.focus_moved != before {
            provider.pane_focus_moved();
        }
        moved
    }

    /// `focus(_:in:)`: focus a pane by id.
    pub fn focus(&mut self, id: &str, list: &[NavPane]) -> bool {
        let target = self.panes(list).into_iter().find(|p| p.id == id);
        match target {
            Some(p) => {
                self.take_focus(p);
                self.focus_moved += 1;
                self.refresh_state(list);
                true
            }
            None => false,
        }
    }

    /// `refresh`: recompute `ringState` from the current focus.
    pub fn refresh_state(&mut self, list: &[NavPane]) {
        let visible = self.panes(list);
        let cur = self.current(list);
        self.ring = if self.window_key && visible.len() > 1 {
            cur.map(|p| RingEntry { pane: p.id.clone(), rect: Some(box_rect(p.rect)) })
        } else {
            None
        };
    }

    /// `testState(_:)`.
    pub fn test_state(&self, list: &[NavPane]) -> PaneTestState {
        let visible = self.panes(list);
        let cur = self.current(list);
        PaneTestState {
            focused: cur.map(|p| p.id.clone()).unwrap_or_default(),
            panes: visible
                .iter()
                .map(|p| PaneEntry { id: p.id.clone(), rect: box_rect(p.rect) })
                .collect(),
            ring: match &self.ring {
                Some(r) => RingEntry { pane: r.pane.clone(), rect: r.rect },
                None => RingEntry { pane: String::new(), rect: None },
            },
        }
    }
}

/// `PaneFocusRing` (`PaneNav.swift:260`): the hairline border drawn around the
/// focused pane. `NSView` subclass (`drawRect:`), macOS-only.
#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBezierPath, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::Cell;

    pub struct PaneFocusRingIvars {
        style: Cell<RingStyle>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPaneFocusRing"]
        #[ivars = PaneFocusRingIvars]
        pub struct PaneFocusRing;

        impl PaneFocusRing {
            #[unsafe(method(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> *mut NSView {
                std::ptr::null_mut()
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                let b = self.bounds();
                let style = self.ivars().style.get();
                let bounds = Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height);
                let path_rect = ring_path_rect(bounds, style.clamped_width());
                let radius = ring_corner_radius(bounds, RING_CORNER_RADIUS);
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    NSRect::new(
                        NSPoint::new(path_rect.x, path_rect.y),
                        NSSize::new(path_rect.width, path_rect.height),
                    ),
                    radius,
                    radius,
                );
                style.rgba().to_nscolor().setStroke();
                path.setLineWidth(style.clamped_width());
                path.stroke();
            }
        }

        unsafe impl NSObjectProtocol for PaneFocusRing {}
    );

    impl PaneFocusRing {
        /// `PaneFocusRing(frame: .zero)` with `wantsLayer = true`, hidden.
        pub fn create(mtm: MainThreadMarker, style: RingStyle) -> Retained<PaneFocusRing> {
            let this = PaneFocusRing::alloc(mtm)
                .set_ivars(PaneFocusRingIvars { style: Cell::new(style) });
            let view: Retained<PaneFocusRing> = unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            };
            view.setWantsLayer(true);
            view.setHidden(true);
            view
        }

        /// `apply(color:width:)`.
        pub fn apply(&self, style: RingStyle) {
            self.ivars().style.set(style);
            self.setNeedsDisplay(true);
        }

        /// Mark the ring for redraw (`drawRect:` paints it).
        pub fn draw(&self) {
            self.setNeedsDisplay(true);
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use appkit::PaneFocusRing;

/// Non-AppKit build: the ring cannot draw.
#[cfg(not(target_os = "macos"))]
pub struct PaneFocusRing;

#[cfg(not(target_os = "macos"))]
impl PaneFocusRing {
    pub fn apply(&self, _style: RingStyle) {}
    pub fn draw(&self) {}
}

/// `layer?.cornerRadius = 6` on the ring view (`PaneNav.swift:264`).
pub const RING_CORNER_RADIUS: f64 = 6.0;

/// `PaneFocusRing.apply(color:width:)` parameter shape, kept as data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RingStyle {
    pub color_argb: u32,
    pub width: f64,
}

impl Default for RingStyle {
    fn default() -> Self {
        // `[app] pane-focus-color` default 8cc8ced8, `pane-focus-width` 1.
        RingStyle { color_argb: 0x8cc8ced8, width: 1.0 }
    }
}

impl RingStyle {
    /// The ring color as `Rgba`; AARRGGBB, going through `hexColor` so the
    /// alpha floor (`0.08`) matches the Swift parser.
    pub fn rgba(&self) -> Rgba {
        HexColor::parse(&format!("{:08X}", self.color_argb)).unwrap_or(Rgba::BLACK)
    }

    /// `kitchen_sink.swift` clamps `pane-focus-width` to `0...4`.
    pub fn clamped_width(&self) -> f64 {
        self.width.clamp(0.0, 4.0)
    }
}

/// The stroked path rect inside `bounds`. The view's frame is already
/// `paneRect.insetBy(0.5, 0.5)`, and a `CALayer` border draws inside its
/// bounds, so inset by half the line width to keep the stroke fully inside.
pub fn ring_path_rect(bounds: Rect, width: f64) -> Rect {
    let inset = (width / 2.0).max(0.0);
    Rect::new(
        bounds.x + inset,
        bounds.y + inset,
        (bounds.width - inset * 2.0).max(0.0),
        (bounds.height - inset * 2.0).max(0.0),
    )
}

/// Corner radius clamped to half the smaller side so the path stays valid.
pub fn ring_corner_radius(bounds: Rect, requested: f64) -> f64 {
    requested.min(bounds.width.min(bounds.height) / 2.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, view: u64, x: f64, y: f64, w: f64, h: f64) -> NavPane {
        NavPane::new(id, ViewId(view), Rect::new(x * 16.0, y * 10.0, w * 16.0, h * 10.0))
    }

    fn notes() -> Vec<NavPane> {
        vec![
            pane("sidebar", 1, 1.0, 9.0, 19.0, 90.0),
            pane("editor", 2, 21.0, 9.0, 78.0, 44.0),
            pane("list", 3, 21.0, 55.0, 41.0, 20.0),
            pane("preview", 4, 63.0, 55.0, 36.0, 20.0),
            pane("terminal", 5, 21.0, 77.0, 78.0, 22.0),
        ]
    }

    #[test]
    fn move_across_layout() {
        let panes = notes();
        let mut nav = PaneNav::new();
        nav.first_responder = Some(ViewId(2));
        assert!(nav.move_dir(PaneDir::Down, &panes));
        assert_eq!(nav.test_state(&panes).focused, "list");
        assert!(nav.move_dir(PaneDir::Right, &panes));
        assert_eq!(nav.test_state(&panes).focused, "preview");
        assert!(nav.move_dir(PaneDir::Down, &panes));
        assert_eq!(nav.test_state(&panes).focused, "terminal");
        assert!(nav.move_dir(PaneDir::Up, &panes));
        // retraces the `came` path (preview -> terminal -> preview)
        assert_eq!(nav.test_state(&panes).focused, "preview");
        assert!(nav.move_dir(PaneDir::Up, &panes));
        assert_eq!(nav.test_state(&panes).focused, "editor");
    }

    #[test]
    fn move_retraces_with_came() {
        let panes = notes();
        let mut nav = PaneNav::new();
        nav.first_responder = Some(ViewId(5));
        assert!(nav.move_dir(PaneDir::Left, &panes));
        assert_eq!(nav.test_state(&panes).focused, "sidebar");
        assert!(nav.move_dir(PaneDir::Right, &panes));
        assert_eq!(nav.test_state(&panes).focused, "terminal");
    }

    #[test]
    fn no_current_focuses_largest() {
        let panes = notes();
        let mut nav = PaneNav::new();
        assert!(nav.move_dir(PaneDir::Down, &panes));
        assert_eq!(nav.test_state(&panes).focused, "editor");
    }

    #[test]
    fn intercept_consumes_direction() {
        let editor = pane("editor", 2, 21.0, 9.0, 78.0, 44.0)
            .with_intercept(|_d| true);
        let list = pane("list", 3, 21.0, 55.0, 41.0, 20.0);
        let panes = vec![editor, list];
        let mut nav = PaneNav::new();
        nav.first_responder = Some(ViewId(2));
        assert!(nav.move_dir(PaneDir::Down, &panes));
        assert_eq!(nav.test_state(&panes).focused, "editor");
        assert_eq!(nav.focus_moved, 0);
    }

    #[test]
    fn focus_by_id_and_state_shape() {
        let panes = notes();
        let mut nav = PaneNav::new();
        assert!(nav.focus("preview", &panes));
        let st = nav.test_state(&panes);
        assert_eq!(st.focused, "preview");
        assert_eq!(st.panes.len(), 5);
        assert_eq!(st.panes[0].id, "sidebar");
        assert_eq!(st.ring.pane, "preview");
        assert!(st.ring.rect.is_some());
        assert!(!nav.focus("nope", &panes));
    }

    #[test]
    fn hidden_panes_are_ignored_and_ring_hidden_when_alone() {
        let mut panes = notes();
        for p in panes.iter_mut() {
            p.visible = p.id == "editor";
        }
        let mut nav = PaneNav::new();
        nav.first_responder = Some(ViewId(2));
        assert_eq!(nav.test_state(&panes).panes.len(), 1);
        nav.refresh_state(&panes);
        assert_eq!(nav.test_state(&panes).ring.pane, "");
        assert!(nav.test_state(&panes).ring.rect.is_none());
    }

    #[test]
    fn provider_callback_fires_on_move() {
        struct P {
            panes: Vec<NavPane>,
            moved: std::cell::Cell<u32>,
        }
        impl PaneProvider for P {
            fn nav_panes(&self) -> Vec<NavPane> {
                self.panes
                    .iter()
                    .map(|p| NavPane::new(&p.id, p.view, p.rect))
                    .collect()
            }
            fn pane_focus_moved(&self) {
                self.moved.set(self.moved.get() + 1);
            }
        }
        let p = P { panes: notes(), moved: std::cell::Cell::new(0) };
        let mut nav = PaneNav::new();
        nav.first_responder = Some(ViewId(2));
        assert!(nav.move_provider(PaneDir::Down, &p));
        assert_eq!(p.moved.get(), 1);
        assert_eq!(nav.first_responder, Some(ViewId(3)));
    }

    #[test]
    fn ring_style_parses_argb() {
        let s = RingStyle::default();
        assert_eq!(s.color_argb, 0x8cc8ced8);
        let c = s.rgba();
        assert!((c.r - 200.0 / 255.0).abs() < 1e-9);
        assert!((c.g - 206.0 / 255.0).abs() < 1e-9);
        assert!((c.b - 216.0 / 255.0).abs() < 1e-9);
        assert!((c.a - 140.0 / 255.0).abs() < 1e-9);
    }

    #[test]
    fn ring_style_alpha_floor() {
        // 0x00... alpha -> hexColor's 0.08 floor.
        let s = RingStyle { color_argb: 0x00c8ced8, width: 1.0 };
        assert!((s.rgba().a - 0.08).abs() < 1e-9);
    }

    #[test]
    fn ring_style_clamps_width() {
        assert_eq!(RingStyle { color_argb: 0, width: -2.0 }.clamped_width(), 0.0);
        assert_eq!(RingStyle { color_argb: 0, width: 9.0 }.clamped_width(), 4.0);
        assert_eq!(RingStyle::default().clamped_width(), 1.0);
    }

    #[test]
    fn ring_path_rect_insets_by_half_width() {
        let b = Rect::new(0.0, 0.0, 100.0, 50.0);
        let r = ring_path_rect(b, 1.0);
        assert_eq!(r, Rect::new(0.5, 0.5, 99.0, 49.0));
        let r2 = ring_path_rect(b, 4.0);
        assert_eq!(r2, Rect::new(2.0, 2.0, 96.0, 46.0));
        // A too-thick stroke must not invert the rect.
        let r3 = ring_path_rect(Rect::new(0.0, 0.0, 2.0, 2.0), 8.0);
        assert_eq!(r3, Rect::new(4.0, 4.0, 0.0, 0.0));
    }

    #[test]
    fn ring_corner_radius_clamps_to_half_side() {
        assert_eq!(ring_corner_radius(Rect::new(0.0, 0.0, 100.0, 50.0), 6.0), 6.0);
        assert_eq!(ring_corner_radius(Rect::new(0.0, 0.0, 4.0, 50.0), 6.0), 2.0);
        assert_eq!(ring_corner_radius(Rect::new(0.0, 0.0, 100.0, -5.0), 6.0), 0.0);
    }
}
