//! `PaneNav.swift`, AppKit half: Ctrl+H/J/K/L over a window's *real* panes.
//!
//! Each view lists its navigable areas as [`LivePane`]s (the Swift
//! `navPanes`); [`LiveNav`] finds the pane owning the window's first
//! responder, picks the neighbour with `pane_geometry::next`, makes it first
//! responder and draws the [`PaneFocusRing`] over it. `state.pane` reports
//! [`LiveNav::test_state`], so probes see real AppKit focus, not a model.

use crate::panes::pane_geometry::{next, remember, Came, PaneDir, PaneRect, Rect};
use crate::panes::pane_nav::{PaneFocusRing, RingStyle};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker, Message};
use objc2_app_kit::{NSButton, NSResponder, NSTextView, NSView, NSWindow, NSWindowOrderingMode};
use objc2_foundation::NSRect;
use serde_json::{json, Value};

/// One navigable area (`NavPane`): `area` is what the ring frames, `focus`
/// the view (or container) that takes the keyboard.
pub struct LivePane {
    pub id: String,
    pub area: Retained<NSView>,
    pub focus: Retained<NSView>,
    /// Ctrl+HJKL moves between nvim splits before leaving this pane.
    pub vim_splits: bool,
}

impl LivePane {
    pub fn new(id: &str, area: &NSView) -> LivePane {
        LivePane { id: id.into(), area: area.retain(), focus: area.retain(), vim_splits: false }
    }

    pub fn focusing(mut self, v: &NSView) -> LivePane {
        self.focus = v.retain();
        self
    }

    fn window_rect(&self) -> NSRect {
        self.area.convertRect_toView(self.area.bounds(), None)
    }

    fn visible(&self, w: &NSWindow) -> bool {
        let same = self.area.window().is_some_and(|aw| std::ptr::eq(&*aw, w));
        if !same || self.area.isHiddenOrHasHiddenAncestor() {
            return false;
        }
        let r = self.window_rect();
        r.size.width > 4.0 && r.size.height > 4.0
    }

    fn contains(&self, r: &NSResponder) -> bool {
        inside(r, &self.area) || inside(r, &self.focus)
    }
}

use crate::ui::view_base::as_any;

/// `NavPane.inside`: the responder is `v`, a descendant, or the field
/// editor of a text field inside `v`.
fn inside(r: &NSResponder, v: &NSView) -> bool {
    if let Some(tv) = as_any(r).downcast_ref::<NSTextView>() {
        if tv.isFieldEditor() {
            let d: Option<Retained<AnyObject>> = unsafe { msg_send![tv, delegate] };
            if let Some(f) = d.as_deref().and_then(|d| d.downcast_ref::<NSView>()) {
                return f.isDescendantOf(v);
            }
        }
    }
    as_any(r).downcast_ref::<NSView>().is_some_and(|rv| rv.isDescendantOf(v))
}

/// `NavPane.firstFocusable(in:)`.
fn first_focusable(v: &NSView) -> Option<Retained<NSView>> {
    if v.acceptsFirstResponder() && !v.isHiddenOrHasHiddenAncestor() && as_any(v).downcast_ref::<NSButton>().is_none() {
        return Some(v.retain());
    }
    for sub in v.subviews().iter() {
        if !sub.isHidden() {
            if let Some(f) = first_focusable(&sub) {
                return Some(f);
            }
        }
    }
    None
}

fn area(r: NSRect) -> f64 {
    r.size.width * r.size.height
}

fn top_down(r: NSRect, w: &NSWindow) -> Rect {
    let h = w.contentView().map(|c| c.bounds().size.height).unwrap_or(w.frame().size.height);
    Rect::new(r.origin.x, h - (r.origin.y + r.size.height), r.size.width, r.size.height)
}

fn boxed(r: Rect) -> [i64; 4] {
    [r.x as i64, r.y as i64, r.width as i64, r.height as i64]
}

/// The window's visible panes, in the provider's order.
fn visible<'a>(w: &NSWindow, list: &'a [LivePane]) -> Vec<&'a LivePane> {
    list.iter().filter(|p| p.visible(w)).collect()
}

/// `current(in:)`: the smallest visible pane owning the first responder.
pub fn current<'a>(w: &NSWindow, list: &'a [LivePane]) -> Option<&'a LivePane> {
    let fr = w.firstResponder()?;
    visible(w, list)
        .into_iter()
        .filter(|p| p.contains(&fr))
        .min_by(|a, b| area(a.window_rect()).partial_cmp(&area(b.window_rect())).unwrap())
}

/// `takeFocus(in:)`.
pub fn take_focus(w: &NSWindow, p: &LivePane) {
    let target = first_focusable(&p.focus).unwrap_or_else(|| p.focus.clone());
    w.makeFirstResponder(Some(&target));
}

/// `PaneNav.shared` state for the shared window.
#[derive(Default)]
pub struct LiveNav {
    came: Came,
    ring: Option<Retained<PaneFocusRing>>,
    ring_state: Option<(String, [i64; 4])>,
    pub style: RingStyle,
}

impl LiveNav {
    /// `move(_:in:)`. `vim_split` asks nvim to `wincmd` first (returns true
    /// when it moved inside nvim).
    pub fn move_dir(
        &mut self,
        w: &NSWindow,
        list: &[LivePane],
        dir: PaneDir,
        vim_split: &dyn Fn(PaneDir) -> bool,
    ) -> bool {
        let vis = visible(w, list);
        if vis.is_empty() {
            return false;
        }
        let Some(cur) = current(w, list) else {
            if let Some(big) = vis
                .iter()
                .max_by(|a, b| area(a.window_rect()).partial_cmp(&area(b.window_rect())).unwrap())
            {
                take_focus(w, big);
            }
            self.refresh(w, list);
            return true;
        };
        if cur.vim_splits && vim_split(dir) {
            return true;
        }
        let rects: Vec<PaneRect> =
            vis.iter().map(|p| PaneRect::new(&p.id, top_down(p.window_rect(), w))).collect();
        if let Some(to) = next(&cur.id, dir, &rects, Some(&self.came)) {
            if let Some(target) = vis.iter().find(|p| p.id == to) {
                remember(&mut self.came, &cur.id, &to, dir);
                take_focus(w, target);
            }
        }
        self.refresh(w, list);
        true
    }

    /// `focus(_:in:)`.
    pub fn focus(&mut self, w: &NSWindow, list: &[LivePane], id: &str) -> bool {
        let Some(p) = visible(w, list).into_iter().find(|p| p.id == id) else {
            return false;
        };
        take_focus(w, p);
        self.refresh(w, list);
        true
    }

    /// `refresh(_:)`: frame the ring on the focused pane (hidden with fewer
    /// than two panes, no focused pane, or a non-key window).
    pub fn refresh(&mut self, w: &NSWindow, list: &[LivePane]) {
        let Some(root) = w.contentView() else { return };
        let mtm = MainThreadMarker::new().expect("main thread");
        let ring = self
            .ring
            .get_or_insert_with(|| PaneFocusRing::create(mtm, RingStyle::default()))
            .clone();
        let attached = unsafe { ring.superview() }.is_some_and(|s| std::ptr::eq(&*s, &*root));
        let vis = visible(w, list);
        let cur = current(w, list);
        let show = w.isKeyWindow() && w.attachedSheet().is_none() && vis.len() > 1 && cur.is_some();
        if !show {
            ring.setHidden(true);
            self.ring_state = None;
            return;
        }
        let cur = cur.unwrap();
        let wr = cur.window_rect();
        let on_top = root.subviews().lastObject().is_some_and(|l| std::ptr::eq(&*l, as_any(&*ring).downcast_ref::<NSView>().unwrap()));
        if !attached || !on_top {
            root.addSubview_positioned_relativeTo(&ring, NSWindowOrderingMode::Above, None);
        }
        let local = root.convertRect_fromView(wr, None);
        let f = NSRect::new(
            objc2_foundation::NSPoint::new(local.origin.x + 0.5, local.origin.y + 0.5),
            objc2_foundation::NSSize::new((local.size.width - 1.0).max(0.0), (local.size.height - 1.0).max(0.0)),
        );
        if ring.frame() != f {
            ring.setFrame(f);
        }
        ring.apply(self.style);
        ring.setHidden(false);
        self.ring_state = Some((cur.id.clone(), boxed(top_down(wr, w))));
    }

    /// `testState(_:)` — `focused` comes from the live first responder.
    pub fn test_state(&self, w: &NSWindow, list: &[LivePane]) -> Value {
        let cur = current(w, list);
        let responder = w
            .firstResponder()
            .map(|r| as_any(&*r).class().name().to_string_lossy().into_owned())
            .unwrap_or_default();
        json!({
            "focused": cur.map(|p| p.id.clone()).unwrap_or_default(),
            "responder": responder,
            "panes": visible(w, list)
                .iter()
                .map(|p| json!({"id": p.id, "rect": boxed(top_down(p.window_rect(), w))}))
                .collect::<Vec<_>>(),
            "ring": match &self.ring_state {
                Some((pane, rect)) => json!({"pane": pane, "rect": rect}),
                None => json!({"pane": ""}),
            },
        })
    }
}
