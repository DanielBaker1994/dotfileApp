//! Display capture, as plain RGBA8 data.
//!
//! The pure annotation model in [`crate::engines::screenshot_annotations`]
//! (`ShotPixels`) consumes a tightly packed RGBA8 buffer; this seam turns a
//! display index into one so a non-AppKit frontend can drive the same engine.
//!
//! macOS: the AppKit / ScreenCaptureKit pipeline in
//! [`crate::views::screenshot`] (`RealCapture` → `sck::capture_displays`)
//! captures displays into `CGImage`s that the overlay paints directly; it is
//! not a raw RGBA buffer and is left exactly as it is. [`capture_screen`] is
//! therefore a deliberate no-op on macOS.
//!
//! Non-macOS: [`xcap`] enumerates monitors (X11 / Wayland) and captures one.

/// A decoded display capture: tightly packed RGBA8, row-major (top row first),
/// no row padding, so `rgba.len() == width * height * 4`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaImage {
    /// Wrap a raw RGBA8 buffer (the layout `xcap` / the `image` crate produce),
    /// rejecting a length that is not exactly `width * height * 4`.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<RgbaImage> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if rgba.len() == expected {
            Some(RgbaImage {
                width,
                height,
                rgba,
            })
        } else {
            None
        }
    }

    /// Bytes per row (RGBA8, no padding).
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    /// Whether `rgba` is exactly `stride() * height` bytes.
    pub fn is_well_formed(&self) -> bool {
        self.rgba.len() == self.stride() * self.height as usize
    }

    /// The RGBA bytes at `(x, y)` in row-major order, or `None` when out of
    /// bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.stride() + x as usize * 4;
        Some([
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ])
    }
}

/// Capture display `display` as a tightly packed RGBA8 image.
///
/// macOS: always `None` — the AppKit path in [`crate::views::screenshot`] uses
/// ScreenCaptureKit's `CGImage`s directly (and is used, unchanged, by the
/// macOS app); there is no RGBA seam here yet.
#[cfg(target_os = "macos")]
pub fn capture_screen(_display: usize) -> Option<RgbaImage> {
    None
}

/// Capture display `display` as a tightly packed RGBA8 image.
///
/// Non-macOS: `xcap` enumerates monitors and captures the `display`-th one (in
/// [`display_count`] order). A missing display, a failed capture, or a headless
/// session with no X11/Wayland display yields `None`.
#[cfg(not(target_os = "macos"))]
pub fn capture_screen(display: usize) -> Option<RgbaImage> {
    let monitors = xcap::Monitor::all().ok()?;
    let monitor = monitors.get(display)?;
    let image = monitor.capture_image().ok()?;
    let (width, height) = image.dimensions();
    RgbaImage::new(width, height, image.into_raw())
}

/// How many displays [`capture_screen`] can address.
///
/// macOS: the `NSScreen` count (0 off the main thread). Non-macOS: the `xcap`
/// monitor count (0 when none can be enumerated, e.g. headless).
#[cfg(target_os = "macos")]
pub fn display_count() -> usize {
    crate::platform::screens().len()
}

/// How many displays [`capture_screen`] can address.
#[cfg(not(target_os = "macos"))]
pub fn display_count() -> usize {
    xcap::Monitor::all().map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_a_tight_rgba_buffer() {
        let img = RgbaImage::new(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.stride(), 8);
        assert!(img.is_well_formed());
    }

    #[test]
    fn new_rejects_a_mismatched_buffer() {
        assert!(RgbaImage::new(2, 2, vec![0; 15]).is_none());
        assert!(RgbaImage::new(2, 2, vec![0; 17]).is_none());
        assert!(RgbaImage::new(2, 2, vec![0; 16]).is_some());
        // The dimension product must not overflow into a bogus length check.
        assert!(RgbaImage::new(u32::MAX, u32::MAX, Vec::new()).is_none());
    }

    #[test]
    fn pixel_is_row_major_top_row_first() {
        let img = RgbaImage::new(
            2,
            2,
            vec![
                10, 11, 12, 13, 14, 15, 16, 17, // row 0
                20, 21, 22, 23, 24, 25, 26, 27, // row 1
            ],
        )
        .unwrap();
        assert_eq!(img.pixel(0, 0), Some([10, 11, 12, 13]));
        assert_eq!(img.pixel(1, 0), Some([14, 15, 16, 17]));
        assert_eq!(img.pixel(0, 1), Some([20, 21, 22, 23]));
        assert_eq!(img.pixel(1, 1), Some([24, 25, 26, 27]));
        assert_eq!(img.pixel(2, 0), None);
        assert_eq!(img.pixel(0, 2), None);
    }

    /// The Linux check image (Docker) has no X11/Wayland display, so `xcap`
    /// cannot enumerate monitors: capture degrades to `None`, never panics.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn capture_screen_is_none_without_a_display() {
        let n = display_count();
        if n == 0 {
            assert_eq!(capture_screen(0), None);
        } else {
            // A real display is present; an out-of-range index is still `None`.
            assert_eq!(capture_screen(n), None);
        }
    }
}
