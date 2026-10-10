//! `SharedWindow.swift`, ported.
//!
//! The `SharedWindow` state machine (`current` / `previousNav` / the back
//! `stack` / `escViews`) is a pure, generic model over a [`SharedWindowHost`]
//! (the `SwitcherController` surface), so every transition is unit-testable
//! without AppKit. The real host (`SlotHostWindow`, a `PopupPlainWindow`-style
//! `NSWindow` subclass) is macOS-gated and only built on demand.
//!
//! First cut: state transitions, the pure geometry (`place` /
//! `screen_containing` / `default_frame`), the `test_state` document and the
//! `SlotHostWindow` class (construction, guest tracking, the "empty hides one
//! turn later" behavior, `take` / `give`). Focus-loss observation, the header
//! decoration wiring and sheet handling land in a later cut.

use crate::app::hotkey::{eval_true_args, AeroCall, AeroIpc};
use crate::app::registry::{nav, RectI, SlotMember, SlotView};
use serde_json::{json, Value};
use std::rc::Rc;
use std::time::Instant;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use std::cell::RefCell;

// ---------------------------------------------------------------------------
// Nav ids + the Esc-close view set (mirrors `SharedWindow.nav*` / `escViews`).
// ---------------------------------------------------------------------------

pub const NAV_NOTES: i64 = nav::NOTES;
pub const NAV_JIRA: i64 = nav::JIRA;
pub const NAV_HOME: i64 = nav::HOME;
pub const NAV_BACK: i64 = nav::BACK;
pub const NAV_FILES: i64 = nav::FILES;
pub const NAV_CONFLUENCE: i64 = nav::CONFLUENCE;
pub const NAV_AI: i64 = nav::AI;
pub const NAV_COMPARE: i64 = nav::COMPARE;

/// `PopupChrome.workspaceBase` — workspace-switch header buttons start here.
pub const WORKSPACE_BASE: i64 = 1000;

pub const ESC_VIEWS: [SlotView; 6] = [
    SlotView::Files,
    SlotView::Notes,
    SlotView::Ai,
    SlotView::Jira,
    SlotView::Confluence,
    SlotView::Compare,
];

pub fn is_esc_view(v: SlotView) -> bool {
    ESC_VIEWS.contains(&v)
}

/// `SharedWindow.navOn(_:)` — the lit nav id, `None` for detail / output.
pub fn nav_on(v: SlotView) -> Option<i64> {
    match v {
        SlotView::Notes => Some(NAV_NOTES),
        SlotView::Files => Some(NAV_FILES),
        SlotView::Confluence => Some(NAV_CONFLUENCE),
        SlotView::Ai => Some(NAV_AI),
        SlotView::Compare | SlotView::CompareText => Some(NAV_COMPARE),
        v if v.is_jira() => Some(NAV_JIRA),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pure geometry (mirrors `SharedWindow.frame` / `screen(of:)` / `place`).
// ---------------------------------------------------------------------------

fn intersection_area(a: RectI, b: RectI) -> f64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0.0 || h <= 0.0 {
        0.0
    } else {
        w * h
    }
}

/// `SharedWindow.screen(of:)` — the screen index whose area covers at least
/// half of `r`, else `None`.
pub fn screen_containing(r: RectI, screens: &[RectI]) -> Option<usize> {
    let area = r.w * r.h;
    if area <= 0.0 {
        return None;
    }
    let mut best: Option<usize> = None;
    let mut best_area = 0.0;
    for (i, s) in screens.iter().enumerate() {
        let a = intersection_area(r, *s);
        if best.is_none() || a > best_area {
            best = Some(i);
            best_area = a;
        }
    }
    let i = best?;
    if best_area >= area / 2.0 {
        Some(i)
    } else {
        None
    }
}

/// `SharedWindow.place(_:from:to:)` — re-centre `r` from one screen's gap
/// frame onto another, clamped inside the target.
pub fn place(r: RectI, from: RectI, to: RectI) -> RectI {
    let w = r.w.min(to.w);
    let h = r.h.min(to.h);
    let mut x = r.x;
    let mut y = r.y;
    if from != to {
        if from.w > 0.0 {
            x = to.x + (r.x + r.w / 2.0 - from.x) / from.w * to.w - w / 2.0;
        }
        if from.h > 0.0 {
            y = to.y + (r.y + r.h / 2.0 - from.y) / from.h * to.h - h / 2.0;
        }
    }
    x = x.max(to.x).min(to.x + to.w - w);
    y = y.max(to.y).min(to.y + to.h - h);
    RectI::new(x, y, w, h)
}

/// The centred default frame for `shared_width` × `shared_height` inside a
/// visible frame (mirrors the `frame` getter's fallback branch).
pub fn default_frame(shared_width: f64, shared_height: f64, visible: RectI) -> RectI {
    let w = shared_width.min(visible.w - 40.0);
    let h = shared_height.min(visible.h - 40.0);
    RectI::new(
        visible.x + visible.w / 2.0 - w / 2.0,
        visible.y + visible.h / 2.0 - h / 2.0,
        w,
        h,
    )
}

// ---------------------------------------------------------------------------
// The controller surface the state machine drives.
// ---------------------------------------------------------------------------

/// The `SwitcherController` + `SlotHostWindow` operations `SharedWindow` needs.
/// Defaults are inert so a test/mock only overrides what it exercises.
pub trait SharedWindowHost {
    /// The live member for a view, when the host exposes one.
    fn member(&self, _v: SlotView) -> Option<Rc<dyn SlotMember>> {
        None
    }
    /// `controller.slotMember(v) != nil`.
    fn has_member(&self, _v: SlotView) -> bool {
        false
    }
    /// `controller.slotMember(v).slotShown`.
    fn is_shown(&self, v: SlotView) -> bool {
        self.member(v).map(|m| m.shown()).unwrap_or(false)
    }
    /// `controller.ensureSlotMember(v, frame:)` — returns false when it fails.
    fn ensure_member(&self, _v: SlotView, _frame: Option<RectI>) -> bool {
        true
    }
    /// Attach + order the member front (`slotAttach` + `slotShow`).
    fn show_member(&self, _v: SlotView, _frame: Option<RectI>) {}
    /// `slotPark(stopVoice:)` — detach + order out.
    fn park_member(&self, _v: SlotView, _stop_voice: bool) {}
    /// `m.slotBaseFrame`.
    fn base_frame(&self, _v: SlotView) -> Option<RectI> {
        None
    }
    /// `m.slotWindow.minSize`.
    fn min_size(&self, _v: SlotView) -> (f64, f64) {
        (0.0, 0.0)
    }
    /// `controller.escHideCount(v)`.
    fn esc_hide_count(&self, _v: SlotView) -> i32 {
        0
    }
    /// `controller.slotShowFiles()`.
    fn show_files(&self) {}
    /// `(controller.savedWID, controller.savedPID)`.
    fn saved_return(&self) -> (Option<String>, Option<i32>) {
        (None, None)
    }
    /// `controller.restoreFocus(wid:pid:)`.
    fn restore_focus(&self, _wid: Option<String>, _pid: Option<i32>) {}
    /// `controller.switchWorkspace(cell:)`.
    fn switch_workspace(&self, _cell: i32) {}
    /// `controller.showViewSwitcher()`.
    fn show_view_switcher(&self) {}
    /// `controller.toggleTerminalPanel()`.
    fn toggle_terminal_panel(&self) {}
    /// `controller.refreshWorkspaceStrip()`.
    fn refresh_workspace_strip(&self) {}
    /// `userInOurWindow` — the focused window really is ours.
    fn user_in_our_window(&self, _v: SlotView) -> bool {
        false
    }
    /// `decorate(_:_:)` — push header buttons/icons onto the member.
    fn decorate_member(&self, _v: SlotView) {}
    fn log(&self, _message: &str) {}
}

/// A host with every default: handy for wiring a `SharedWindow` before a real
/// controller exists.
#[derive(Default)]
pub struct NoHost;

impl SharedWindowHost for NoHost {}

// ---------------------------------------------------------------------------
// The state machine.
// ---------------------------------------------------------------------------

pub struct SharedWindow<H: SharedWindowHost> {
    host: H,
    current: Option<SlotView>,
    last: SlotView,
    last_jira: SlotView,
    previous_nav: Option<i64>,
    stack: Vec<SlotView>,
    guest: Option<SlotView>,
    target_screen: Option<i32>,
    return_wid: Option<String>,
    return_pid: Option<i32>,
    summoned: bool,
    swapped_at: Option<Instant>,
    aerospace_cache_cleared: bool,
    frame_rect: RectI,
    frame_key: String,
    shared_width: f64,
    shared_height: f64,
    host_shown: bool,
    palette_visible: bool,
    aero: Option<Box<dyn AeroCall>>,
    #[cfg(target_os = "macos")]
    host_window: RefCell<Option<Retained<SlotHostWindow>>>,
}

impl<H: SharedWindowHost> SharedWindow<H> {
    pub fn new(host: H) -> Self {
        Self::with_frame_key(host, "sharedWindowFrame")
    }

    pub fn with_frame_key(host: H, frame_key: &str) -> Self {
        SharedWindow {
            host,
            current: None,
            last: SlotView::Notes,
            last_jira: SlotView::Jira,
            previous_nav: None,
            stack: Vec::new(),
            guest: None,
            target_screen: None,
            return_wid: None,
            return_pid: None,
            summoned: false,
            swapped_at: None,
            aerospace_cache_cleared: false,
            frame_rect: RectI::new(0.0, 0.0, 900.0, 600.0),
            frame_key: frame_key.to_string(),
            shared_width: 1100.0,
            shared_height: 640.0,
            host_shown: false,
            palette_visible: false,
            aero: None,
            #[cfg(target_os = "macos")]
            host_window: RefCell::new(None),
        }
    }

    /// Inject the AeroSpace IPC used by [`Self::present`]'s cache clear.
    pub fn with_aero(mut self, ipc: Box<dyn AeroCall>) -> Self {
        self.aero = Some(ipc);
        self
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn frame_key(&self) -> &str {
        &self.frame_key
    }

    pub fn set_shared_size(&mut self, width: f64, height: f64) {
        self.shared_width = width;
        self.shared_height = height;
    }

    pub fn current(&self) -> Option<SlotView> {
        self.current
    }

    pub fn last(&self) -> SlotView {
        self.last
    }

    pub fn previous_nav(&self) -> Option<i64> {
        self.previous_nav
    }

    pub fn stack(&self) -> &[SlotView] {
        &self.stack
    }

    pub fn guest(&self) -> Option<SlotView> {
        self.guest
    }

    pub fn is_host_shown(&self) -> bool {
        self.host_shown
    }

    pub fn target_screen(&self) -> Option<i32> {
        self.target_screen
    }

    pub fn set_target_screen(&mut self, index: Option<i32>) {
        self.target_screen = index;
    }

    /// `targetNSScreen` — the 0-based screen index from the 1-based one.
    pub fn target_ns_screen(&self) -> Option<usize> {
        self.target_screen.filter(|i| *i >= 1).map(|i| (i - 1) as usize)
    }

    pub fn frame(&self) -> RectI {
        self.frame_rect
    }

    pub fn set_frame(&mut self, r: RectI) {
        self.frame_rect = r;
    }

    pub fn current_frame(&self) -> RectI {
        self.shown_member()
            .and_then(|v| self.host.base_frame(v))
            .unwrap_or(self.frame_rect)
    }

    pub fn is_visible(&self) -> bool {
        self.shown_member().is_some()
    }

    pub fn shown_member(&self) -> Option<SlotView> {
        self.current.filter(|v| self.host.is_shown(*v))
    }

    /// `hotkey(_:userInIt:)`.
    pub fn hotkey(&mut self, v: SlotView, user_in_it: Option<bool>) {
        if let Some(cur) = self.current {
            let same = if v == SlotView::Jira { cur.is_jira() } else { cur == v };
            if same && self.shown_member().is_some() {
                self.hide_or_focus(cur, user_in_it);
                return;
            }
        }
        if v == SlotView::Jira && self.last_jira != SlotView::Jira && self.host.has_member(self.last_jira) {
            self.present(self.last_jira, None);
        } else {
            self.open(v);
        }
    }

    /// `toggle(userInIt:)`.
    pub fn toggle(&mut self, user_in_it: Option<bool>) {
        if let Some(m) = self.shown_member() {
            self.hide_or_focus(m, user_in_it);
            return;
        }
        if self.host.has_member(self.last) {
            self.present(self.last, None);
            return;
        }
        let top = if self.last.is_jira() { SlotView::Jira } else { self.last };
        if top == SlotView::Files {
            self.host.show_files();
        } else {
            self.open(top);
        }
        if !self.is_visible() && top != SlotView::Files {
            self.host.show_files();
        }
    }

    /// `open(_:)`.
    pub fn open(&mut self, v: SlotView) {
        if v == SlotView::Jira && self.current.map_or(false, |c| c.is_jira()) {
            self.stack.clear();
        }
        if !v.is_jira() {
            self.stack.retain(|x| *x != v);
        }
        if !self.host.ensure_member(v, Some(self.current_frame())) {
            return;
        }
        self.present(v, None);
    }

    /// `push(_:)`.
    pub fn push(&mut self, v: SlotView) {
        if let Some(cur) = self.current {
            if self.is_visible() {
                if cur != v {
                    self.stack.push(cur);
                }
            } else {
                self.stack = if v.is_jira() {
                    vec![SlotView::Jira]
                } else if v == SlotView::CompareText {
                    vec![SlotView::Compare]
                } else {
                    Vec::new()
                };
            }
        } else {
            self.stack = if v.is_jira() {
                vec![SlotView::Jira]
            } else if v == SlotView::CompareText {
                vec![SlotView::Compare]
            } else {
                Vec::new()
            };
        }
        self.stack.retain(|x| *x != v);
        self.present(v, None);
    }

    /// `back(esc:)`.
    pub fn back(&mut self, esc: bool) {
        while let Some(prev) = self.stack.pop() {
            if self.host.has_member(prev) || prev == SlotView::Jira || prev == SlotView::Compare {
                self.open(prev);
                return;
            }
        }
        if let Some(cur) = self.current {
            if cur.is_jira() && cur != SlotView::Jira {
                self.open(SlotView::Jira);
            } else if cur == SlotView::CompareText {
                self.open(SlotView::Compare);
            } else if esc {
                self.escape_at_top(cur);
            } else {
                self.hide("back from the first view", true);
            }
        }
    }

    /// `home()`.
    pub fn home(&mut self) {
        self.stack.clear();
        self.open(SlotView::Jira);
    }

    /// `escapeAtTop(_:)`.
    pub fn escape_at_top(&mut self, v: SlotView) {
        if self.host.esc_hide_count(v) > 0 {
            self.hide(&format!("Esc ({})", v.raw()), true);
        }
    }

    /// `memberGone(_:)`.
    pub fn member_gone(&mut self, v: SlotView) {
        self.stack.retain(|x| *x != v);
        if self.current == Some(v) {
            self.current = None;
        }
        if self.guest == Some(v) {
            self.guest = None;
        }
        if self.last == v {
            self.last = if v.is_jira() {
                SlotView::Jira
            } else if v == SlotView::CompareText {
                SlotView::Compare
            } else {
                SlotView::Files
            };
        }
        if self.last_jira == v {
            self.last_jira = SlotView::Jira;
        }
    }

    /// `cycle(_:)` — logs the direction, then nav-clicks the fixed next id.
    pub fn cycle(&mut self, dir: i32) {
        let next = if self.current == Some(SlotView::Files) {
            NAV_NOTES
        } else {
            NAV_FILES
        };
        self.host.log(&format!(
            "cycle: current={} dir={} next={}",
            self.current.map(|v| v.raw()).unwrap_or("nil"),
            dir,
            next
        ));
        self.nav_clicked(next);
    }

    /// `navClicked(_:)`.
    pub fn nav_clicked(&mut self, id: i64) {
        match id {
            NAV_NOTES => {
                if self.current != Some(SlotView::Notes) {
                    self.open(SlotView::Notes);
                }
            }
            NAV_FILES => {
                if self.current != Some(SlotView::Files) {
                    self.host.show_files();
                }
            }
            NAV_JIRA => {
                if self.current.map_or(false, |c| c.is_jira()) {
                    self.home();
                } else {
                    self.hotkey(SlotView::Jira, None);
                }
            }
            NAV_CONFLUENCE => {
                if self.current != Some(SlotView::Confluence) {
                    self.open(SlotView::Confluence);
                }
            }
            NAV_AI => {
                if self.current != Some(SlotView::Ai) {
                    self.open(SlotView::Ai);
                }
            }
            NAV_COMPARE => {
                if self.current != Some(SlotView::Compare) {
                    self.open(SlotView::Compare);
                }
            }
            NAV_HOME => self.home(),
            NAV_BACK => self.back(false),
            id if id >= WORKSPACE_BASE => self.host.switch_workspace((id - WORKSPACE_BASE) as i32),
            _ => {}
        }
    }

    /// `present(_:)` — `override_frame` mirrors the internal `let f = …`
    /// computed from `currentFrame()`.
    pub fn present(&mut self, v: SlotView, override_frame: Option<RectI>) {
        if !self.host.ensure_member(v, Some(self.current_frame())) {
            return;
        }
        if let Some(cur) = self.current {
            if let (Some(a), Some(b)) = (nav_on(cur), nav_on(v)) {
                if a != b {
                    self.previous_nav = Some(a);
                }
            }
        }
        if !self.summoned {
            self.summoned = true;
            let (wid, pid) = self.host.saved_return();
            self.return_wid = wid;
            self.return_pid = pid;
        }
        let mut f = override_frame.unwrap_or_else(|| self.current_frame());
        let mut outgoing: Option<SlotView> = None;
        if let Some(old) = self.shown_member() {
            if old != v {
                if let Some(bf) = self.host.base_frame(old) {
                    self.frame_rect = bf;
                }
                outgoing = Some(old);
            }
        }
        let host_up = self.host_shown;
        if let Some(old) = outgoing {
            self.host.park_member(old, false);
        }
        if let Some(g) = self.guest {
            if g != v {
                self.host.park_member(g, false);
            }
        }
        let (min_w, min_h) = self.host.min_size(v);
        if f.w < min_w {
            f.w = min_w;
        }
        if f.h < min_h {
            f.y -= min_h - f.h;
            f.h = min_h;
        }
        self.frame_rect = f;
        self.host.decorate_member(v);
        self.host.refresh_workspace_strip();
        if !host_up && !self.aerospace_cache_cleared {
            if let Some(ipc) = self.aero.as_deref() {
                let _ = ipc.call(&eval_true_args());
            }
        }
        self.aerospace_cache_cleared = false;
        self.host.show_member(v, Some(f));
        if outgoing.is_some() {
            self.swapped_at = Some(Instant::now());
        }
        self.current = Some(v);
        self.guest = Some(v);
        self.last = v;
        if v.is_jira() {
            self.last_jira = v;
        }
        self.host_shown = true;
        let suffix = if self.stack.is_empty() {
            String::new()
        } else {
            let back: Vec<&str> = self.stack.iter().map(|x| x.raw()).collect();
            format!(" (back: {})", back.join(" > "))
        };
        self.host.log(&format!("shared window: {}{}", v.raw(), suffix));
    }

    /// `prepare(_:)` — set the frame + decorate without showing.
    pub fn prepare(&mut self, v: SlotView) {
        if self.host.is_shown(v) {
            return;
        }
        self.host.decorate_member(v);
    }

    /// `hide(_:restoreFocus:)`.
    pub fn hide(&mut self, reason: &str, restore_focus: bool) {
        self.summoned = false;
        self.swapped_at = None;
        self.target_screen = None;
        self.aerospace_cache_cleared = false;
        let Some(cur) = self.current else {
            if self.host_shown {
                self.order_out_host();
                self.host.log(&format!("shared window: hidden{}", reason_suffix(reason)));
            }
            return;
        };
        self.current = None;
        if self.host.is_shown(cur) {
            if let Some(bf) = self.host.base_frame(cur) {
                self.frame_rect = bf;
            }
        }
        self.host.park_member(cur, true);
        if let Some(g) = self.guest {
            self.host.park_member(g, true);
        }
        self.guest = None;
        self.order_out_host();
        if restore_focus {
            self.host.restore_focus(self.return_wid.clone(), self.return_pid);
        }
        self.return_wid = None;
        self.return_pid = None;
        self.host.log(&format!(
            "shared window: hidden ({}){}",
            cur.raw(),
            reason_suffix(reason)
        ));
    }

    fn hide_or_focus(&mut self, v: SlotView, user_in_it: Option<bool>) {
        let in_it = user_in_it.unwrap_or_else(|| self.host.user_in_our_window(v));
        if in_it {
            self.hide("hotkey pressed while in it", true);
        } else {
            self.host.show_member(v, None);
        }
    }

    /// `SlotHostWindow.onEmpty` → the view went away.
    pub fn on_host_empty(&mut self) {
        self.hide("its view went away", false);
    }

    pub fn set_palette_visible(&mut self, visible: bool) {
        self.palette_visible = visible;
    }

    fn order_out_host(&mut self) {
        self.host_shown = false;
        #[cfg(target_os = "macos")]
        if let Some(h) = self.host_window.borrow().as_ref() {
            h.orderOut(None);
        }
    }

    /// `testQuery`'s shared-window state — the top-level `view` / `visible` /
    /// `windows` / `palette` keys.
    pub fn test_state(&self) -> Value {
        let view = self.current.map(|v| v.raw());
        json!({
            "view": view,
            "visible": self.is_visible(),
            "windows": [{
                "kind": "shared",
                "view": view,
                "visible": self.is_visible(),
                "hostShown": self.host_shown,
                "targetScreen": self.target_screen,
            }],
            "palette": { "visible": self.palette_visible },
        })
    }
}

fn reason_suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(" — {reason}")
    }
}

#[cfg(target_os = "macos")]
impl<H: SharedWindowHost> SharedWindow<H> {
    /// Build the AppKit host window on first use.
    pub fn ensure_host_window(&self, mtm: MainThreadMarker) -> Retained<SlotHostWindow> {
        if let Some(h) = self.host_window.borrow().as_ref() {
            return h.clone();
        }
        let h = SlotHostWindow::create(mtm);
        *self.host_window.borrow_mut() = Some(h.clone());
        h
    }

    pub fn host_window(&self) -> Option<Retained<SlotHostWindow>> {
        self.host_window.borrow().clone()
    }
}

/// `SharedWindow.clearAerospaceCache()`.
pub fn clear_aerospace_cache() {
    let ipc = AeroIpc::discover();
    clear_aerospace_cache_with(&ipc);
}

pub fn clear_aerospace_cache_with(ipc: &dyn AeroCall) {
    let _ = ipc.call(&eval_true_args());
}

// ---------------------------------------------------------------------------
// The AppKit host window (`SlotHostWindow`, mirrors `SharedWindow.swift`).
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub use appkit::SlotHostWindow;

#[cfg(target_os = "macos")]
mod appkit {
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::AnyObject;
    use objc2::{
        define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSAppearanceCustomization, NSBackingStoreType, NSColor, NSView, NSWindow, NSWindowAnimationBehavior,
        NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};

    use crate::ui::card::CardNSWindow;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    pub struct SlotHostWindowIvars {
        corner_radius: Cell<f64>,
        guest: RefCell<Option<Weak<AnyObject>>>,
        on_empty: RefCell<Option<Box<dyn Fn()>>>,
        on_close_request: RefCell<Option<Box<dyn Fn()>>>,
    }

    define_class!(
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSlotHostWindow"]
        #[ivars = SlotHostWindowIvars]
        pub struct SlotHostWindow;

        impl SlotHostWindow {
            #[unsafe(method(_cornerRadius))]
            fn _corner_radius(&self) -> f64 {
                self.ivars().corner_radius.get()
            }

            #[unsafe(method(performClose:))]
            fn perform_close(&self, _sender: Option<&AnyObject>) {
                if let Some(f) = self.ivars().on_close_request.borrow().as_ref() {
                    f();
                }
            }

            #[unsafe(method(close))]
            fn close(&self) {
                if let Some(f) = self.ivars().on_close_request.borrow().as_ref() {
                    f();
                }
            }

            #[unsafe(method(fireHostEmpty))]
            fn fire_host_empty(&self) {
                if self.ivars().guest.borrow().is_none() && self.isVisible() {
                    if let Some(f) = self.ivars().on_empty.borrow().as_ref() {
                        f();
                    }
                }
            }
        }

        unsafe impl NSObjectProtocol for SlotHostWindow {}
    );

    impl SlotHostWindow {
        pub fn create(mtm: MainThreadMarker) -> Retained<SlotHostWindow> {
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0));
            let mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::FullSizeContentView;
            let this = SlotHostWindow::alloc(mtm).set_ivars(SlotHostWindowIvars {
                corner_radius: Cell::new(10.0),
                guest: RefCell::new(None),
                on_empty: RefCell::new(None),
                on_close_request: RefCell::new(None),
            });
            let window: Retained<SlotHostWindow> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: mask,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            window.setTitlebarAppearsTransparent(true);
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            for b in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(btn) = window.standardWindowButton(b) {
                    btn.setHidden(true);
                }
            }
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(true);
            unsafe { window.setReleasedWhenClosed(false) };
            window.setAnimationBehavior(NSWindowAnimationBehavior::None);
            window.setAcceptsMouseMovedEvents(true);
            let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
            window.setContentView(Some(&content));
            window
        }

        pub fn corner_radius(&self) -> f64 {
            self.ivars().corner_radius.get()
        }

        pub fn set_corner_radius(&self, r: f64) {
            self.ivars().corner_radius.set(r);
            self.invalidateShadow();
        }

        pub fn set_on_empty(&self, cb: Box<dyn Fn()>) {
            *self.ivars().on_empty.borrow_mut() = Some(cb);
        }

        pub fn set_on_close_request(&self, cb: Box<dyn Fn()>) {
            *self.ivars().on_close_request.borrow_mut() = Some(cb);
        }

        pub fn guest(&self) -> Option<Retained<AnyObject>> {
            self.ivars().guest.borrow().as_ref().and_then(|w| w.load())
        }

        /// `take(_:chromeOf:)` — track the guest and copy the card chrome.
        pub fn take(&self, g: &AnyObject, chrome_of: &NSWindow) {
            let w = chrome_of;
            *self.ivars().guest.borrow_mut() = Some(Weak::new(g));
            if let Some(c) = as_any(w).downcast_ref::<CardNSWindow>() {
                self.set_corner_radius(c.corner_radius());
            }
            self.setTitle(&w.title());
            self.setMinSize(w.minSize());
            self.setAppearance(w.appearance().as_deref());
            self.setHasShadow(w.hasShadow());
            self.setCollectionBehavior(w.collectionBehavior());
            if let (Some(mine), Some(theirs)) = (
                self.standardWindowButton(NSWindowButton::CloseButton),
                w.standardWindowButton(NSWindowButton::CloseButton),
            ) {
                mine.setHidden(theirs.isHidden());
            }
            self.invalidateShadow();
        }

        /// `give(chromeTo:)` — hand the chrome back to the member window.
        pub fn give(&self, chrome_to: &NSWindow) {
            let w = chrome_to;
            if let Some(c) = as_any(w).downcast_ref::<CardNSWindow>() {
                c.set_corner_radius(self.corner_radius());
            }
            w.setAppearance(self.appearance().as_deref());
        }

        /// `guestLeft(_:)` — drop the weak guest, then hide one main-loop turn
        /// later if nobody reattached.
        pub fn guest_left(&self, g: &AnyObject) {
            let matches = self
                .ivars()
                .guest
                .borrow()
                .as_ref()
                .and_then(|w| w.load())
                .map(|cur| std::ptr::eq(Retained::as_ptr(&cur), g as *const AnyObject))
                .unwrap_or(false);
            if !matches {
                return;
            }
            *self.ivars().guest.borrow_mut() = None;
            unsafe {
                let _: () = msg_send![
                    self,
                    performSelector: sel!(fireHostEmpty),
                    withObject: Option::<&AnyObject>::None,
                    afterDelay: 0.0f64
                ];
            }
        }

        /// `moveContent(from:to:)` — swap the two windows' content roots.
        pub fn move_content(mtm: MainThreadMarker, from: &NSWindow, to: &NSWindow) {
            let Some(root) = from.contentView() else {
                return;
            };
            from.setContentView(Some(&NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            )));
            to.setContentView(Some(&root));
        }
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct MockHost {
        present: RefCell<HashSet<SlotView>>,
        shown: RefCell<HashSet<SlotView>>,
        frames: RefCell<HashMap<SlotView, RectI>>,
        min: RefCell<HashMap<SlotView, (f64, f64)>>,
        esc: RefCell<HashMap<SlotView, i32>>,
        saved: RefCell<(Option<String>, Option<i32>)>,
        show_files_calls: Cell<u32>,
        restored: RefCell<Vec<(Option<String>, Option<i32>)>>,
        decorated: RefCell<Vec<SlotView>>,
        logs: RefCell<Vec<String>>,
    }

    impl MockHost {
        fn with_esc(self, v: SlotView, n: i32) -> Self {
            self.esc.borrow_mut().insert(v, n);
            self
        }
        fn with_saved(self, wid: &str, pid: i32) -> Self {
            *self.saved.borrow_mut() = (Some(wid.to_string()), Some(pid));
            self
        }
    }

    impl SharedWindowHost for MockHost {
        fn has_member(&self, v: SlotView) -> bool {
            self.present.borrow().contains(&v)
        }
        fn is_shown(&self, v: SlotView) -> bool {
            self.shown.borrow().contains(&v)
        }
        fn ensure_member(&self, v: SlotView, _frame: Option<RectI>) -> bool {
            self.present.borrow_mut().insert(v);
            true
        }
        fn show_member(&self, v: SlotView, _frame: Option<RectI>) {
            self.present.borrow_mut().insert(v);
            self.shown.borrow_mut().insert(v);
        }
        fn park_member(&self, v: SlotView, _stop_voice: bool) {
            self.shown.borrow_mut().remove(&v);
        }
        fn base_frame(&self, v: SlotView) -> Option<RectI> {
            self.frames.borrow().get(&v).copied()
        }
        fn min_size(&self, v: SlotView) -> (f64, f64) {
            self.min.borrow().get(&v).copied().unwrap_or((0.0, 0.0))
        }
        fn esc_hide_count(&self, v: SlotView) -> i32 {
            self.esc.borrow().get(&v).copied().unwrap_or(0)
        }
        fn show_files(&self) {
            self.show_files_calls.set(self.show_files_calls.get() + 1);
        }
        fn user_in_our_window(&self, v: SlotView) -> bool {
            self.is_shown(v)
        }
        fn saved_return(&self) -> (Option<String>, Option<i32>) {
            self.saved.borrow().clone()
        }
        fn restore_focus(&self, wid: Option<String>, pid: Option<i32>) {
            self.restored.borrow_mut().push((wid, pid));
        }
        fn decorate_member(&self, v: SlotView) {
            self.decorated.borrow_mut().push(v);
        }
        fn log(&self, m: &str) {
            self.logs.borrow_mut().push(m.to_string());
        }
    }

    fn window() -> SharedWindow<MockHost> {
        SharedWindow::new(MockHost::default())
    }

    #[test]
    fn open_presents_and_tracks() {
        let mut w = window();
        w.open(SlotView::Notes);
        assert_eq!(w.current(), Some(SlotView::Notes));
        assert_eq!(w.last(), SlotView::Notes);
        assert!(w.is_visible());
        assert!(w.is_host_shown());
        assert_eq!(w.guest(), Some(SlotView::Notes));
        assert!(w.host().shown.borrow().contains(&SlotView::Notes));
    }

    #[test]
    fn toggle_hides_when_shown() {
        let mut w = window();
        w.open(SlotView::Notes);
        w.toggle(None);
        assert_eq!(w.current(), None);
        assert!(!w.is_visible());
        assert!(!w.is_host_shown());
    }

    #[test]
    fn toggle_reopens_last() {
        let mut w = window();
        w.open(SlotView::Notes);
        w.hide("", true);
        assert!(!w.is_visible());
        w.toggle(None);
        assert_eq!(w.current(), Some(SlotView::Notes));
        assert!(w.is_visible());
    }

    #[test]
    fn hotkey_same_view_hides_or_focuses() {
        let mut w = window();
        w.open(SlotView::Notes);
        w.hotkey(SlotView::Notes, Some(true));
        assert_eq!(w.current(), None);

        w.open(SlotView::Notes);
        w.hotkey(SlotView::Notes, Some(false));
        assert_eq!(w.current(), Some(SlotView::Notes));
        assert!(w.is_visible());
    }

    #[test]
    fn present_sets_previous_nav_and_parks_outgoing() {
        let mut w = window();
        w.present(SlotView::Notes, None);
        assert_eq!(w.previous_nav(), None);
        w.present(SlotView::Files, None);
        assert_eq!(w.previous_nav(), Some(NAV_NOTES));
        assert_eq!(w.current(), Some(SlotView::Files));
        assert!(!w.host().shown.borrow().contains(&SlotView::Notes));
        assert!(w.host().shown.borrow().contains(&SlotView::Files));
    }

    #[test]
    fn back_returns_via_stack() {
        let mut w = window();
        w.open(SlotView::Jira);
        w.push(SlotView::Detail);
        assert_eq!(w.current(), Some(SlotView::Detail));
        assert_eq!(w.stack(), &[SlotView::Jira]);
        w.back(false);
        assert_eq!(w.current(), Some(SlotView::Jira));
        assert!(w.stack().is_empty());
    }

    #[test]
    fn back_from_jira_subview_goes_home() {
        let mut w = window();
        w.open(SlotView::Config);
        assert_eq!(w.current(), Some(SlotView::Config));
        w.back(false);
        assert_eq!(w.current(), Some(SlotView::Jira));
    }

    #[test]
    fn back_from_compare_text_goes_to_compare() {
        let mut w = window();
        w.open(SlotView::CompareText);
        w.back(false);
        assert_eq!(w.current(), Some(SlotView::Compare));
    }

    #[test]
    fn back_esc_at_top_hides_only_when_configured() {
        let mut w = SharedWindow::new(MockHost::default().with_esc(SlotView::Notes, 1));
        w.open(SlotView::Notes);
        w.back(true);
        assert_eq!(w.current(), None);

        let mut w = window();
        w.open(SlotView::Notes);
        w.back(true);
        assert_eq!(w.current(), Some(SlotView::Notes));
    }

    #[test]
    fn back_without_esc_hides_at_first_view() {
        let mut w = window();
        w.open(SlotView::Notes);
        w.back(false);
        assert_eq!(w.current(), None);
        assert_eq!(w.host().restored.borrow().len(), 1);
    }

    #[test]
    fn cycle_from_notes_shows_files() {
        let mut w = window();
        w.open(SlotView::Notes);
        w.cycle(1);
        assert_eq!(w.host().show_files_calls.get(), 1);
        assert_eq!(w.current(), Some(SlotView::Notes));
    }

    #[test]
    fn cycle_from_files_opens_notes() {
        let mut w = window();
        w.open(SlotView::Files);
        w.cycle(1);
        assert_eq!(w.current(), Some(SlotView::Notes));
    }

    #[test]
    fn member_gone_adjusts_last_and_current() {
        let mut w = window();
        w.open(SlotView::CompareText);
        w.member_gone(SlotView::CompareText);
        assert_eq!(w.current(), None);
        assert_eq!(w.last(), SlotView::Compare);
    }

    #[test]
    fn empty_host_hides_without_restoring_focus() {
        let mut w = SharedWindow::new(MockHost::default().with_saved("42", 555));
        w.open(SlotView::Notes);
        w.on_host_empty();
        assert_eq!(w.current(), None);
        assert!(!w.is_visible());
        assert!(w.host().restored.borrow().is_empty());
    }

    #[test]
    fn hide_restores_focus_to_saved_target() {
        let mut w = SharedWindow::new(MockHost::default().with_saved("42", 555));
        w.open(SlotView::Notes);
        w.hide("x", true);
        let restored = w.host().restored.borrow();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0], (Some("42".to_string()), Some(555)));
    }

    #[test]
    fn esc_views_membership() {
        for v in [
            SlotView::Files,
            SlotView::Notes,
            SlotView::Ai,
            SlotView::Jira,
            SlotView::Confluence,
            SlotView::Compare,
        ] {
            assert!(is_esc_view(v), "{v:?}");
        }
        for v in [
            SlotView::Detail,
            SlotView::Releases,
            SlotView::Config,
            SlotView::Output,
            SlotView::CompareText,
        ] {
            assert!(!is_esc_view(v), "{v:?}");
        }
    }

    #[test]
    fn test_state_shape() {
        let mut w = window();
        w.open(SlotView::Notes);
        let s = w.test_state();
        assert_eq!(s["view"], json!("notes"));
        assert_eq!(s["visible"], json!(true));
        assert!(s["windows"].is_array());
        assert_eq!(s["windows"][0]["kind"], json!("shared"));
        assert_eq!(s["palette"]["visible"], json!(false));

        w.set_palette_visible(true);
        assert_eq!(w.test_state()["palette"]["visible"], json!(true));
    }

    #[test]
    fn place_centres_and_clamps() {
        let from = RectI::new(0.0, 0.0, 1000.0, 1000.0);
        let to = RectI::new(2000.0, 0.0, 1000.0, 1000.0);
        let r = RectI::new(100.0, 100.0, 400.0, 300.0);
        let p = place(r, from, to);
        assert_eq!(p.w, 400.0);
        assert_eq!(p.h, 300.0);
        assert_eq!(p.x, 2100.0);
        assert_eq!(p.y, 100.0);

        // Oversized -> clamped to the target.
        let big = RectI::new(0.0, 0.0, 5000.0, 5000.0);
        let p = place(big, from, to);
        assert_eq!(p.x, 2000.0);
        assert_eq!(p.w, 1000.0);
    }

    #[test]
    fn screen_containing_needs_half_coverage() {
        let screens = [
            RectI::new(0.0, 0.0, 1440.0, 900.0),
            RectI::new(1440.0, 0.0, 1440.0, 900.0),
        ];
        assert_eq!(
            screen_containing(RectI::new(0.0, 0.0, 100.0, 100.0), &screens),
            Some(0)
        );
        assert_eq!(
            screen_containing(RectI::new(1500.0, 200.0, 400.0, 300.0), &screens),
            Some(1)
        );
        // Mostly off the union of screens -> no majority.
        assert_eq!(
            screen_containing(RectI::new(2800.0, 0.0, 200.0, 80.0), &screens),
            None
        );
        assert_eq!(screen_containing(RectI::new(0.0, 0.0, 0.0, 0.0), &screens), None);
    }

    #[test]
    fn default_frame_centres_and_caps() {
        let vis = RectI::new(0.0, 0.0, 1440.0, 900.0);
        let f = default_frame(1100.0, 640.0, vis);
        assert_eq!(f.w, 1100.0);
        assert_eq!(f.h, 640.0);
        assert_eq!(f.x, 170.0);
        assert_eq!(f.y, 130.0);

        let huge = default_frame(4000.0, 4000.0, vis);
        assert_eq!(huge.w, 1400.0);
        assert_eq!(huge.h, 860.0);
    }

    #[test]
    fn nav_clicked_routes() {
        let mut w = window();
        w.nav_clicked(NAV_NOTES);
        assert_eq!(w.current(), Some(SlotView::Notes));
        w.push(SlotView::Ai);
        assert_eq!(w.current(), Some(SlotView::Ai));
        w.nav_clicked(NAV_BACK);
        assert_eq!(w.current(), Some(SlotView::Notes));
    }
}
