//! Display geometry, as plain data (was direct `NSScreen` calls).
//!
//! Coordinates keep AppKit's convention: the origin is the bottom-left corner
//! of the primary display, y grows upwards, and secondary displays sit at
//! whatever offset the user arranged them at (`frame` may have negative x/y).
//! Nothing is flipped here.
//!
//! `NSScreen` is main-thread-only, so the macOS backend takes a
//! `MainThreadMarker` itself and reports "nothing" (`Vec::new()` / `None`) off
//! the main thread; non-macOS targets always report "nothing". When a call
//! site needs an `NSRect` from a [`Rect`], it builds one there:
//! `NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))`.

#[cfg(target_os = "macos")]
use objc2_app_kit::NSScreen;

/// A rectangle in AppKit's global display space (bottom-left origin, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
}

/// One display as plain data.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenInfo {
    /// The `NSScreenNumber` display id (from `deviceDescription`).
    pub id: u32,
    /// The full frame.
    pub frame: Rect,
    /// The visible frame (menu bar and Dock removed).
    pub visible: Rect,
    /// The backing scale (`backingScaleFactor`).
    pub scale: f64,
}

/// The screen whose frame contains `(x, y)`, AppKit's half-open convention
/// (left/bottom edges inclusive, right/top edges exclusive).
fn containing(screens: &[ScreenInfo], x: f64, y: f64) -> Option<ScreenInfo> {
    screens
        .iter()
        .find(|s| {
            x >= s.frame.x
                && x < s.frame.x + s.frame.w
                && y >= s.frame.y
                && y < s.frame.y + s.frame.h
        })
        .cloned()
}

/// Every display in `NSScreen.screens` order. Empty off the main thread.
/// Screens without an `NSScreenNumber` are skipped (as the old
/// `current_screens` did).
#[cfg(target_os = "macos")]
pub fn screens() -> Vec<ScreenInfo> {
    use objc2::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    NSScreen::screens(mtm).iter().filter_map(|s| info(&s)).collect()
}

/// The main screen (`NSScreen.mainScreen`). `None` off the main thread.
#[cfg(target_os = "macos")]
pub fn main_screen() -> Option<ScreenInfo> {
    use objc2::MainThreadMarker;
    let scr = NSScreen::mainScreen(MainThreadMarker::new()?)?;
    info(&scr)
}

/// The screen whose frame contains `(x, y)`, else `None`.
#[cfg(target_os = "macos")]
pub fn screen_containing(x: f64, y: f64) -> Option<ScreenInfo> {
    containing(&screens(), x, y)
}

#[cfg(target_os = "macos")]
fn info(scr: &NSScreen) -> Option<ScreenInfo> {
    use objc2_foundation::{NSNumber, NSString};
    let key = NSString::from_str("NSScreenNumber");
    let id = scr
        .deviceDescription()
        .objectForKey(&key)
        .and_then(|o| o.downcast::<NSNumber>().ok())?
        .unsignedIntValue();
    let f = scr.frame();
    let v = scr.visibleFrame();
    Some(ScreenInfo {
        id,
        frame: Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
        visible: Rect::new(v.origin.x, v.origin.y, v.size.width, v.size.height),
        scale: scr.backingScaleFactor(),
    })
}

/// Every display. Non-macOS: always empty.
#[cfg(not(target_os = "macos"))]
pub fn screens() -> Vec<ScreenInfo> {
    Vec::new()
}

/// The main screen. Non-macOS: always `None`.
#[cfg(not(target_os = "macos"))]
pub fn main_screen() -> Option<ScreenInfo> {
    None
}

/// The screen containing `(x, y)`. Non-macOS: always `None`.
#[cfg(not(target_os = "macos"))]
pub fn screen_containing(x: f64, y: f64) -> Option<ScreenInfo> {
    containing(&[], x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: u32, x: f64, y: f64, w: f64, h: f64) -> ScreenInfo {
        ScreenInfo {
            id,
            frame: Rect::new(x, y, w, h),
            visible: Rect::new(x, y, w, h),
            scale: 2.0,
        }
    }

    #[test]
    fn containing_picks_the_screen_under_the_point() {
        let screens = [
            screen(1, 0.0, 0.0, 1440.0, 900.0),
            screen(2, 1440.0, -200.0, 1920.0, 1080.0),
        ];
        assert_eq!(containing(&screens, 10.0, 10.0).map(|s| s.id), Some(1));
        // A secondary display can sit below/left of the primary (negative y).
        assert_eq!(containing(&screens, 1500.0, -100.0).map(|s| s.id), Some(2));
        assert_eq!(containing(&screens, 10_000.0, 0.0), None);
    }

    #[test]
    fn containing_edges_are_half_open() {
        let screens = [
            screen(1, 0.0, 0.0, 1440.0, 900.0),
            screen(2, 1440.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(containing(&screens, 0.0, 0.0).map(|s| s.id), Some(1));
        assert_eq!(containing(&screens, 1439.0, 899.0).map(|s| s.id), Some(1));
        assert_eq!(containing(&screens, 1440.0, 0.0).map(|s| s.id), Some(2));
        // Right/top edges are exclusive; nothing there when no screen starts
        // exactly on the other side.
        assert_eq!(containing(&screens, 1439.0, 900.0), None);
        assert_eq!(containing(&[], 0.0, 0.0), None);
    }

    /// `NSScreen` is main-thread-only; the test harness' worker threads get
    /// `None`, so this can only assert on a main-thread run.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs a live GUI session on the main thread"]
    fn live_main_screen_is_well_formed() {
        let Some(s) = main_screen() else { return };
        assert!(s.frame.w > 0.0 && s.frame.h > 0.0);
        assert!(s.visible.w > 0.0 && s.visible.h > 0.0);
        assert!(s.scale > 0.0);
    }
}
