//! Port of `Screenshot.swift` + `ScreenshotOverlay.swift` + `ScreenshotPin.swift`.
//!
//! Bounded first cut. The annotation model (`ShotTool`, `ShotDocument`,
//! `ButtonRing`, `ShotPixelate`, `ShotState`, `ShotArgs`, `ShotFiles`…) already
//! lives in [`crate::engines::screenshot_annotations`]; this module owns the
//! controller, the delivery model and the overlay session STATE machine.
//!
//! Real here: the `[screenshot]` config, the permission externs
//! (`CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`), the
//! `do:screenshot:*` hook dispatch, the controller `testState` JSON,
//! `ShotOutcome` delivery (copy/save/pin model), `ShotHistory` (recents +
//! pruning), the `PinPanel` model, `ScreenToast`, and the `ShotSession` Esc
//! chain / text mode / tool selection / key routing.
//!
//! Real: the ScreenCaptureKit capture (`SCShareableContent` →
//! `SCContentFilter` + `SCStreamConfiguration` → `SCScreenshotManager`,
//! permission-gated; see [`RealCapture`]).
//!
//! Real: the JPEG encode (`shot_cg::reencode_png_as_jpeg`) and the pasteboard
//! writes (`copy`), and the live overlay — one borderless screen-level
//! `ShotOverlayPanel` per display with a `ShotOverlayView` that paints the
//! frozen capture, the dim mask outside the selection, the selection border +
//! eight resize handles, and the text draft, driven by mouse events into the
//! `ShotSession` state machine (see the `overlay` module).
//!
//! Real: the local `NSEvent` keyDown monitor (`ScreenshotController::
//! install_key_monitor` / `drop_key_monitor_if_idle`) — installed while a
//! session or a pin is live, it routes keyDowns whose window is a
//! `ShotOverlayPanel` into `ShotSession::handle_key` and ones whose window is a
//! pin into `PinPanel::handle_key`, consuming handled keys and passing the rest
//! through.
//!
//! Real: the OCR image path — [`crate::engines::screenshot_text::ShotOCR`] runs
//! Vision `VNRecognizeTextRequest` over the rendered capture (honouring the
//! `[screenshot] text-languages` / `text-correction` config), driven from
//! `deliver_text` and warmed by `prewarm`; a missing permission, an off-main
//! call or a non-decodable image degrades to the "no text" path, never an error.
//!
//! Real: the overlay chrome — the `ShotOverlayView` render pass paints the help
//! card, the button ring, the right-click colour wheel, the loupe and the save
//! card alongside the core surface (dim/selection/handles/text draft). The
//! chrome *layout* is ported as pure, unit-tested models (`ShotWheel`,
//! `ShotLoupe`, `ShotHelpCard`, `ShotSaveCard`, `ShotSidePanel`,
//! `ShotRecentButton`, `ShotModePill`, `ShotToolTab`, `ShotSizeIndicator`).
//!
//! `todo!()`: the interactive AppKit controls the Swift subviews embed — the
//! side-panel settings form (sliders/steppers/table) and the editable
//! save-card field — keep their layout models only; their SF Symbol glyphs and
//! editable controls are not rebuilt in the Rust overlay.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app::registry::{PaletteCommand, Registry};
use crate::engines::screenshot_annotations::{
    round_half_away, ButtonRing, Point, Rect, ShotArgs, ShotColor, ShotDate, ShotDocument,
    ShotFiles, ShotGeom, ShotMode, ShotObject, ShotSnap, ShotState, ShotTextStyle, ShotTool, Size,
};

/// The frozen base image behind a [`ShotDisplay`] (macOS only; `()` elsewhere).
#[cfg(target_os = "macos")]
pub type BaseImage = objc2::rc::Retained<objc2_core_graphics::CGImage>;
#[cfg(not(target_os = "macos"))]
pub type BaseImage = ();

/// The backing scale `ShotCanvas.init` derives: `img.width / max(1, frame.width)`,
/// clamped to ≥ 1 (the ×2/×1 rule).
pub fn canvas_scale(image_width: i64, frame_width: f64) -> f64 {
    let s = image_width as f64 / frame_width.max(1.0);
    if s > 1.0 {
        s
    } else {
        1.0
    }
}

/// `CGRect.integral`: floor the origin, round the far edge up.
pub fn integral(r: Rect) -> Rect {
    let x0 = r.x.floor();
    let y0 = r.y.floor();
    let x1 = (r.x + r.w).ceil();
    let y1 = (r.y + r.h).ceil();
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// `ShotCanvas.pxRect`: a point rect to the base image's integral pixel rect.
pub fn px_rect(r: Rect, scale: f64) -> Rect {
    integral(Rect::new(r.x * scale, r.y * scale, r.w * scale, r.h * scale))
}

/// The output bitmap size: crop (points) × backing scale, rounded.
pub fn output_size(crop: Rect, scale: f64) -> (i64, i64) {
    (
        round_half_away(crop.w * scale) as i64,
        round_half_away(crop.h * scale) as i64,
    )
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

pub const DEFAULT_USER_COLORS: &str =
    "picker, #800000, #ff0000, #ffff00, #00ff00, #008000, #00ffff, #0000ff, #ff00ff, #800080";

pub const HELP_KEYS: [(&str, &str, &str); 8] = [
    ("help-mouse", "Mouse", "Select screenshot area"),
    ("help-save", "\u{2318}S", "Save screenshot to a file"),
    ("help-copy", "\u{2318}C", "Copy selection to clipboard"),
    ("help-wheel", "Mouse Wheel", "Change tool size"),
    ("help-right-click", "Right Click", "Show color picker"),
    ("help-space", "Space", "Open side panel"),
    ("help-copy-text", "Tab", "Copy text mode"),
    ("help-esc", "Esc", "Exit"),
];

pub const HELP_TEXT_KEYS: [(&str, &str, &str); 3] = [
    ("help-text", "Mouse", "Drag over text to copy it"),
    ("help-text-mode", "Tab", "Back to screenshot mode"),
    ("help-esc", "Esc", "Exit"),
];

fn hex(s: &str) -> ShotColor {
    ShotColor::from_hex(s).unwrap_or(ShotColor::BLACK)
}

fn tri(s: Option<&str>) -> Option<bool> {
    match s.map(str::to_lowercase).as_deref() {
        Some("true") | Some("yes") | Some("1") | Some("on") => Some(true),
        Some("false") | Some("no") | Some("0") | Some("off") => Some(false),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotOCRConfigModel {
    pub languages: Vec<String>,
    pub correction: bool,
}

impl Default for ShotOCRConfigModel {
    fn default() -> Self {
        ShotOCRConfigModel {
            languages: Vec::new(),
            correction: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotConfig {
    pub enabled: bool,
    pub ui_color: ShotColor,
    pub contrast_color: ShotColor,
    pub contrast_opacity: i64,
    pub draw_color: ShotColor,
    pub user_colors: Vec<Option<ShotColor>>,
    pub buttons: Vec<ShotTool>,
    pub button_size: f64,
    pub show_help: bool,
    pub show_side_panel_button: bool,
    pub magnifier: bool,
    pub square_magnifier: bool,
    pub copy_on_double_click: bool,
    pub return_action: String,
    pub save_path: String,
    pub save_path_fixed: bool,
    pub filename_pattern: String,
    pub save_format: String,
    pub jpeg_quality: i64,
    pub save_after_copy: bool,
    pub history: i64,
    pub copy_path_after_save: bool,
    pub preview: bool,
    pub save_last_region: bool,
    pub undo_limit: i64,
    pub arrow_style: i64,
    pub reverse_arrow: bool,
    pub counter_outline: bool,
    pub insecure_pixelate: bool,
    pub delay_ms: i64,
    pub font: String,
    pub copy_toast: String,
    pub save_toast: String,
    pub permission_toast: String,
    pub fail_toast: String,
    pub help_rows: Vec<(String, String)>,
    pub start_text: bool,
    pub ocr: ShotOCRConfigModel,
    pub text_toast: String,
    pub no_text_toast: String,
    pub help_text_rows: Vec<(String, String)>,
    pub shortcuts: Vec<(String, String)>,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        ScreenshotConfig {
            enabled: true,
            ui_color: hex("#740096"),
            contrast_color: hex("#270032"),
            contrast_opacity: 190,
            draw_color: hex("#ff0000"),
            user_colors: vec![None],
            buttons: ShotTool::ring("", true),
            button_size: 34.0,
            show_help: true,
            show_side_panel_button: true,
            magnifier: false,
            square_magnifier: false,
            copy_on_double_click: false,
            return_action: "copy".to_string(),
            save_path: "~/Desktop".to_string(),
            save_path_fixed: false,
            filename_pattern: "%F_%H-%M".to_string(),
            save_format: "png".to_string(),
            jpeg_quality: 75,
            save_after_copy: false,
            history: 20,
            copy_path_after_save: false,
            preview: true,
            save_last_region: false,
            undo_limit: 100,
            arrow_style: 0,
            reverse_arrow: false,
            counter_outline: true,
            insecure_pixelate: false,
            delay_ms: 0,
            font: String::new(),
            copy_toast: "Capture saved to clipboard".to_string(),
            save_toast: "Capture saved as {}".to_string(),
            permission_toast: "Screen Recording permission needed \u{2014} opening Settings\u{2026}"
                .to_string(),
            fail_toast: "Screen capture failed".to_string(),
            help_rows: HELP_KEYS
                .iter()
                .map(|(_, label, text)| (label.to_string(), text.to_string()))
                .collect(),
            start_text: false,
            ocr: ShotOCRConfigModel::default(),
            text_toast: "Copied {} to clipboard".to_string(),
            no_text_toast: "No text found".to_string(),
            help_text_rows: HELP_TEXT_KEYS
                .iter()
                .map(|(_, label, text)| (label.to_string(), text.to_string()))
                .collect(),
            shortcuts: Vec::new(),
        }
    }
}

impl ScreenshotConfig {
    /// `ScreenshotConfig.load()` — reads `[screenshot]` from commands.toml.
    pub fn load() -> ScreenshotConfig {
        let Some(text) = crate::app::config::read_config_text() else {
            return ScreenshotConfig::default();
        };
        let lines = crate::engines::config_text::config_lines(&text);
        let entries = crate::engines::config_text::config_section_entries(&lines, "screenshot");
        let mut map: HashMap<String, String> = HashMap::new();
        for (_, k, v) in entries {
            map.insert(k, v);
        }
        ScreenshotConfig::from_entries(&map)
    }

    /// Pure mirror of the `load()` key parsing (python-free, testable).
    pub fn from_entries(e: &HashMap<String, String>) -> ScreenshotConfig {
        let d = ScreenshotConfig::default();
        let b = |k: &str, dv: bool| tri(e.get(k).map(String::as_str)).unwrap_or(dv);
        let i = |k: &str, dv: i64, lo: i64, hi: i64| -> i64 {
            e.get(k)
                .and_then(|s| s.trim().parse::<f64>().ok())
                .map(|v| (v as i64).clamp(lo, hi))
                .unwrap_or(dv)
        };
        let s = |k: &str, dv: &str| -> String {
            match e.get(k) {
                Some(v) if !v.is_empty() => v.clone(),
                _ => dv.to_string(),
            }
        };
        let col = |k: &str, dv: ShotColor| -> ShotColor {
            match e.get(k).map(|v| v.trim()) {
                Some(v) if !v.is_empty() => {
                    if v.eq_ignore_ascii_case("theme") {
                        dv
                    } else {
                        ShotColor::from_hex(v).unwrap_or(dv)
                    }
                }
                _ => dv,
            }
        };

        let mut c = ScreenshotConfig {
            enabled: b("enabled", true),
            ui_color: col("ui-color", d.ui_color),
            contrast_color: col("contrast-color", d.contrast_color),
            contrast_opacity: i("contrast-opacity", 190, 0, 255),
            draw_color: col("draw-color", d.draw_color),
            ..d.clone()
        };

        let uc = s("user-colors", DEFAULT_USER_COLORS);
        c.user_colors = uc
            .split(',')
            .filter_map(|part| {
                let v = part.trim();
                if v.eq_ignore_ascii_case("picker") {
                    Some(None)
                } else {
                    ShotColor::from_hex(v).map(Some)
                }
            })
            .collect();

        c.buttons = ShotTool::ring(&s("buttons", ""), b("show-size-badge", true));
        let bs = i("button-size", 0, 0, 80);
        c.button_size = if bs >= 20 {
            bs as f64
        } else {
            ButtonRing::default_button_size(15.5)
        };
        c.show_help = b("show-help", true);
        c.show_side_panel_button = b("show-side-panel-button", true);
        c.magnifier = b("magnifier", false);
        c.square_magnifier = b("square-magnifier", false);
        c.copy_on_double_click = b("copy-on-double-click", false);
        let r = s("return", "copy").to_lowercase();
        c.return_action = if ["copy", "save", "pin"].contains(&r.as_str()) {
            r
        } else {
            "copy".to_string()
        };
        c.history = i("history", 20, 0, 500);
        c.save_path = s("save-path", "~/Desktop");
        c.save_path_fixed = b("save-path-fixed", false);
        c.filename_pattern = s("filename-pattern", "%F_%H-%M");
        let f = s("save-format", "png").to_lowercase();
        c.save_format = if ["jpg", "jpeg"].contains(&f.as_str()) {
            "jpg".to_string()
        } else {
            "png".to_string()
        };
        c.jpeg_quality = i("jpeg-quality", 75, 1, 100);
        c.save_after_copy = b("save-after-copy", false);
        c.copy_path_after_save = b("copy-path-after-save", false);
        c.preview = b("preview", true);
        c.save_last_region = b("save-last-region", false);
        c.undo_limit = i("undo-limit", 100, 1, 1000);
        c.arrow_style = i("arrow-style", 0, 0, 1);
        c.reverse_arrow = b("reverse-arrow", false);
        c.counter_outline = b("counter-outline", true);
        c.insecure_pixelate = b("insecure-pixelate", false);
        c.delay_ms = i("delay", 0, 0, 60_000);
        c.font = e.get("font").cloned().unwrap_or_default();
        if let Some(v) = e.get("copy-toast") {
            c.copy_toast = v.clone();
        }
        if let Some(v) = e.get("save-toast") {
            c.save_toast = v.clone();
        }
        c.permission_toast = s("permission-toast", &d.permission_toast);
        c.fail_toast = s("fail-toast", &d.fail_toast);
        c.help_rows = HELP_KEYS
            .iter()
            .map(|(key, label, text)| (label.to_string(), s(key, text)))
            .collect();
        c.help_text_rows = HELP_TEXT_KEYS
            .iter()
            .map(|(key, label, text)| (label.to_string(), s(key, text)))
            .collect();
        c.start_text = s("start-mode", "screenshot").to_lowercase() == "text";
        c.ocr.languages = s("text-languages", "")
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        c.ocr.correction = b("text-correction", true);
        if let Some(v) = e.get("text-toast") {
            c.text_toast = v.clone();
        }
        c.no_text_toast = s("no-text-toast", &d.no_text_toast);
        c
    }

    pub fn palette_command(&self) -> PaletteCommand {
        PaletteCommand::new("screenshot", "/screenshot", "tools")
    }
}

// __NEXT__

// ---------------------------------------------------------------------------
// Screen capture — ScreenCaptureKit (macOS) with an unavailable fallback.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRef {
    pub id: u32,
    pub frame: Rect,
    pub scale: f64,
}

impl ScreenRef {
    pub fn new(id: u32, frame: Rect, scale: f64) -> Self {
        ScreenRef { id, frame, scale }
    }
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.frame.w, self.frame.h)
    }
}

#[derive(Clone)]
pub struct CapturedImage {
    pub display: u32,
    pub width: i64,
    pub height: i64,
    pub scale: f64,
    #[cfg(target_os = "macos")]
    pub image: Option<objc2::rc::Retained<objc2_core_graphics::CGImage>>,
}

impl std::fmt::Debug for CapturedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedImage")
            .field("display", &self.display)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaptureError(pub String);

impl CaptureError {
    pub fn new(msg: impl Into<String>) -> Self {
        CaptureError(msg.into())
    }
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CaptureError {}

/// The Swift `SCShareableContent` + `SCScreenshotManager.captureImage` flow.
pub trait ScreenCapture {
    fn shareable_content(&self) -> Result<(), CaptureError>;
    fn capture(&self, screens: &[ScreenRef]) -> Result<Vec<CapturedImage>, CaptureError>;
}

/// Fallback backend (no Screen Recording permission, or a non-macOS build):
/// every call fails with a clear note. [`RealCapture`] is used when capture is
/// permitted.
pub struct UnavailableCapture;

impl ScreenCapture for UnavailableCapture {
    fn shareable_content(&self) -> Result<(), CaptureError> {
        Err(CaptureError::new(
            "ScreenCaptureKit unavailable: SCShareableContent not captured",
        ))
    }
    fn capture(&self, _screens: &[ScreenRef]) -> Result<Vec<CapturedImage>, CaptureError> {
        Err(CaptureError::new(
            "ScreenCaptureKit unavailable: SCScreenshotManager not captured",
        ))
    }
}

/// The Swift `SCShareableContent` + `SCScreenshotManager.captureImage` flow:
/// enumerate displays, then capture each (points × backing scale, no cursor)
/// with a per-display completion handler resolved through a blocking channel.
#[cfg(target_os = "macos")]
pub struct RealCapture;

#[cfg(target_os = "macos")]
impl ScreenCapture for RealCapture {
    fn shareable_content(&self) -> Result<(), CaptureError> {
        sck::fetch_shareable_content().map(|_| ())
    }
    fn capture(&self, screens: &[ScreenRef]) -> Result<Vec<CapturedImage>, CaptureError> {
        sck::capture_displays(screens)
    }
}

#[cfg(target_os = "macos")]
mod sck {
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_core_graphics::CGImage;
    use objc2_foundation::{NSArray, NSError};
    use objc2_screen_capture_kit::{
        SCCaptureResolutionType, SCContentFilter, SCDisplay, SCScreenshotManager,
        SCShareableContent, SCStreamConfiguration, SCWindow,
    };

    use super::{CaptureError, CapturedImage, ScreenRef};

    const TIMEOUT: Duration = Duration::from_secs(15);

    type ShareableResult = Result<Retained<SCShareableContent>, CaptureError>;
    type ImageResult = Result<Retained<CGImage>, CaptureError>;

    fn error_text(error: *mut NSError) -> String {
        if error.is_null() {
            return "ScreenCaptureKit error".to_string();
        }
        unsafe { (*error).localizedDescription().to_string() }
    }

    fn shareable_content_block(
    ) -> (
        RcBlock<dyn Fn(*mut SCShareableContent, *mut NSError)>,
        Receiver<ShareableResult>,
    ) {
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if !error.is_null() {
                Err(CaptureError::new(error_text(error)))
            } else {
                unsafe { Retained::retain(content) }
                    .ok_or_else(|| CaptureError::new("SCShareableContent was nil"))
            };
            let _ = tx.send(result);
        });
        (block, rx)
    }

    fn capture_block(
    ) -> (
        RcBlock<dyn Fn(*mut CGImage, *mut NSError)>,
        Receiver<ImageResult>,
    ) {
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
            let result = if !error.is_null() {
                Err(CaptureError::new(error_text(error)))
            } else {
                unsafe { Retained::retain(image) }
                    .ok_or_else(|| CaptureError::new("SCScreenshotManager image was nil"))
            };
            let _ = tx.send(result);
        });
        (block, rx)
    }

    pub fn fetch_shareable_content() -> ShareableResult {
        let (block, rx) = shareable_content_block();
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                false,
                true,
                &block,
            );
        }
        rx.recv_timeout(TIMEOUT)
            .map_err(|_| CaptureError::new("SCShareableContent timed out"))?
    }

    fn match_display(displays: &NSArray<SCDisplay>, id: u32) -> Option<Retained<SCDisplay>> {
        displays
            .iter()
            .find(|display| unsafe { display.displayID() } == id)
    }

    pub fn capture_displays(screens: &[ScreenRef]) -> Result<Vec<CapturedImage>, CaptureError> {
        if screens.is_empty() {
            return Err(CaptureError::new("no screens to capture"));
        }
        let content = fetch_shareable_content()?;
        let displays = unsafe { content.displays() };
        let excluded = NSArray::<SCWindow>::new();

        let mut pending: Vec<(
            u32,
            f64,
            RcBlock<dyn Fn(*mut CGImage, *mut NSError)>,
            Receiver<ImageResult>,
        )> = Vec::new();
        for screen in screens {
            let Some(display) = match_display(&displays, screen.id) else {
                continue;
            };
            let filter = unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            };
            let config = unsafe { SCStreamConfiguration::new() };
            let width = (screen.frame.w * screen.scale).round().max(1.0) as usize;
            let height = (screen.frame.h * screen.scale).round().max(1.0) as usize;
            unsafe {
                config.setWidth(width);
                config.setHeight(height);
                config.setShowsCursor(false);
                config.setCaptureResolution(SCCaptureResolutionType::Best);
            }
            let (block, rx) = capture_block();
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&block),
                );
            }
            pending.push((screen.id, screen.scale, block, rx));
        }

        let mut out = Vec::new();
        for (display, scale, _block, rx) in pending {
            if let Ok(Ok(image)) = rx.recv_timeout(TIMEOUT) {
                out.push(CapturedImage {
                    display,
                    width: CGImage::width(Some(&image)) as i64,
                    height: CGImage::height(Some(&image)) as i64,
                    scale,
                    image: Some(image),
                });
            }
        }
        if out.is_empty() {
            return Err(CaptureError::new("no displays captured"));
        }
        Ok(out)
    }
}

extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// `NSScreen.screens` as [`ScreenRef`]s: the `NSScreenNumber` display id, the
/// AppKit frame (the overlay panels use it as-is) and the backing scale.
/// Empty off the main thread.
#[cfg(target_os = "macos")]
pub fn current_screens() -> Vec<ScreenRef> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::{NSNumber, NSString};
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return Vec::new();
    };
    let key = NSString::from_str("NSScreenNumber");
    NSScreen::screens(mtm)
        .iter()
        .filter_map(|scr| {
            let id = scr
                .deviceDescription()
                .objectForKey(&key)
                .and_then(|o| o.downcast::<NSNumber>().ok())?
                .unsignedIntValue();
            let f = scr.frame();
            Some(ScreenRef::new(
                id,
                Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
                scr.backingScaleFactor(),
            ))
        })
        .collect()
}

pub fn screen_capture_permitted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn request_screen_capture_access() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

// ---------------------------------------------------------------------------
// ScreenToast
// ---------------------------------------------------------------------------

pub const TOAST_DURATION: f64 = 1.4;

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenToast {
    pub text: String,
    pub symbol: String,
    pub duration: f64,
}

impl ScreenToast {
    pub fn new(text: impl Into<String>) -> Self {
        ScreenToast::with_symbol(text, "checkmark.circle.fill")
    }
    pub fn with_symbol(text: impl Into<String>, symbol: impl Into<String>) -> Self {
        ScreenToast {
            text: text.into(),
            symbol: symbol.into(),
            duration: TOAST_DURATION,
        }
    }
    /// `ScreenToast.show` — an empty text is dropped, like the Swift guard.
    pub fn show(slot: &mut Option<ScreenToast>, text: impl Into<String>, symbol: &str) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        *slot = Some(ScreenToast::with_symbol(text, symbol));
    }
}

// ---------------------------------------------------------------------------
// ShotHistory
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotRecentEntry {
    pub path: String,
    pub name: String,
    pub at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotHistory {
    pub dir: String,
}

impl Default for ShotHistory {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        ShotHistory {
            dir: format!("{home}/.cache/kitchen-sink/screenshots"),
        }
    }
}

impl ShotHistory {
    pub fn new(dir: impl Into<String>) -> Self {
        ShotHistory { dir: dir.into() }
    }

    pub fn entries(&self) -> Vec<ShotRecentEntry> {
        ShotHistory::entries_in(&self.dir)
    }

    /// Newest first, by name (`shot-YYYYMMDD-HHmmss-SSS.png` sorts by time).
    pub fn entries_in(dir: &str) -> Vec<ShotRecentEntry> {
        let mut names: Vec<String> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("shot-") && n.ends_with(".png"))
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names.reverse();
        names
            .into_iter()
            .map(|n| {
                let path = format!("{}/{n}", dir.trim_end_matches('/'));
                let at = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_else(unix_now);
                ShotRecentEntry { path, name: n, at }
            })
            .collect()
    }

    /// Write a PNG and prune everything past `limit`. Returns the new path.
    pub fn record_png(&self, bytes: &[u8], limit: usize) -> Option<String> {
        if limit == 0 {
            return None;
        }
        ShotHistory::record_png_at(&self.dir, bytes, &ShotHistory::now_name(), limit)
    }

    pub fn record_png_at(dir: &str, bytes: &[u8], name: &str, limit: usize) -> Option<String> {
        if limit == 0 {
            return None;
        }
        std::fs::create_dir_all(dir).ok()?;
        let path = format!("{}/{name}", dir.trim_end_matches('/'));
        std::fs::write(&path, bytes).ok()?;
        ShotHistory::prune_in(dir, limit);
        Some(path)
    }

    /// Remove `shot-*.png` past `limit` (newest kept). Returns removed paths.
    pub fn prune_in(dir: &str, limit: usize) -> Vec<String> {
        let removed: Vec<String> = ShotHistory::entries_in(dir)
            .into_iter()
            .skip(limit)
            .map(|e| e.path)
            .collect();
        for p in &removed {
            let _ = std::fs::remove_file(p);
        }
        removed
    }

    pub fn now_name() -> String {
        now_stamp()
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `yyyyMMdd-HHmmss-SSS` in local time (the Swift directory-entry name).
fn now_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let d = ShotDate::from_unix_local(now.as_secs() as i64);
    format!(
        "shot-{:04}{:02}{:02}-{:02}{:02}{:02}-{:03}.png",
        d.year,
        d.month,
        d.day,
        d.hour,
        d.minute,
        d.second,
        now.subsec_millis()
    )
}

// __NEXT__

// ---------------------------------------------------------------------------
// PinPanel
// ---------------------------------------------------------------------------

pub const PIN_MARGIN: f64 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub struct PinPanel {
    pub base: Size,
    pub zoom: f64,
    pub alpha: f64,
    pub quarter_turns: i64,
    pub frame: Rect,
    pub key: bool,
    pub window_number: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinKeyAction {
    Close,
    Copy,
    SetAlpha,
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinKey {
    pub key_code: u16,
    pub ch: Option<char>,
    pub cmd: bool,
}

impl PinPanel {
    pub fn new(base: Size, frame: Rect) -> Self {
        PinPanel {
            base,
            zoom: 1.0,
            alpha: 1.0,
            quarter_turns: 0,
            frame,
            key: false,
            window_number: 0,
        }
    }

    pub fn set_zoom(&mut self, z: f64, around: Option<Point>) {
        let b = self.base;
        let min_z = 100.0 / 1.0f64.max(b.w.min(b.h));
        self.zoom = 1.0f64.min(min_z).max(8.0f64.min(z));
        let size = (
            (b.w * self.zoom).round() + PIN_MARGIN * 2.0,
            (b.h * self.zoom).round() + PIN_MARGIN * 2.0,
        );
        let c = around.unwrap_or(Point::new(self.frame.mid_x(), self.frame.mid_y()));
        self.frame = Rect::new(
            (c.x - size.0 / 2.0).round(),
            (c.y - size.1 / 2.0).round(),
            size.0,
            size.1,
        );
    }

    pub fn rotate(&mut self, quarters: i64) {
        if quarters % 2 != 0 {
            self.base = Size::new(self.base.h, self.base.w);
        }
        self.quarter_turns = 0;
        self.set_zoom(self.zoom, None);
    }

    pub fn opacity(&mut self, delta: f64) {
        self.alpha = 0.1f64.max(1.0f64.min(self.alpha + delta));
    }

    /// Mirrors `PinPanel.handleKey` (Esc / Cmd+W+Q close, Cmd+C copies, 0-9
    /// sets opacity, everything else is swallowed).
    pub fn handle_key(&mut self, k: &PinKey) -> PinKeyAction {
        let ch = k.ch.map(|c| c.to_ascii_lowercase());
        if k.key_code == 53 || (k.cmd && matches!(ch, Some('w') | Some('q'))) {
            return PinKeyAction::Close;
        }
        if k.cmd && ch == Some('c') {
            return PinKeyAction::Copy;
        }
        if !k.cmd {
            if let Some(d) = ch.and_then(|c| c.to_digit(10)) {
                self.alpha = if d == 0 { 1.0 } else { (10 - d) as f64 / 10.0 };
                return PinKeyAction::SetAlpha;
            }
        }
        PinKeyAction::Ignored
    }

    /// The right-click menu titles (mirrors `PinPanel.menu`).
    pub fn menu_titles() -> Vec<&'static str> {
        vec![
            "Copy to clipboard",
            "Save to file",
            "-",
            "Rotate Right",
            "Rotate Left",
            "Increase Opacity",
            "Decrease Opacity",
            "-",
            "Close",
        ]
    }
}

// ---------------------------------------------------------------------------
// Outcome + image
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotOutcome {
    Copy,
    Save,
    Pin,
    Accept,
    Abort,
    Text,
}

impl ShotOutcome {
    pub fn raw_value(self) -> &'static str {
        match self {
            ShotOutcome::Copy => "copy",
            ShotOutcome::Save => "save",
            ShotOutcome::Pin => "pin",
            ShotOutcome::Accept => "accept",
            ShotOutcome::Abort => "abort",
            ShotOutcome::Text => "text",
        }
    }
    pub fn from_raw(s: &str) -> Option<ShotOutcome> {
        Some(match s {
            "copy" => ShotOutcome::Copy,
            "save" => ShotOutcome::Save,
            "pin" => ShotOutcome::Pin,
            "accept" => ShotOutcome::Accept,
            "abort" => ShotOutcome::Abort,
            "text" => ShotOutcome::Text,
            _ => return None,
        })
    }
}

/// A rendered capture. `png` holds the encoded bytes the pasteboard / save
/// path uses (PNG via `shot_cg::encode_png`; JPEG is re-encoded on save).
#[derive(Clone, Debug, PartialEq)]
pub struct ShotImage {
    pub width: i64,
    pub height: i64,
    pub png: Option<Vec<u8>>,
}

impl ShotImage {
    pub fn new(width: i64, height: i64) -> Self {
        ShotImage {
            width,
            height,
            png: None,
        }
    }
    pub fn with_png(width: i64, height: i64, png: Vec<u8>) -> Self {
        ShotImage {
            width,
            height,
            png: Some(png),
        }
    }
}

/// `finish`'s accept-action resolution (pure; mirrors the Swift switch).
pub fn resolve_action(o: ShotOutcome, args: &ShotArgs, cfg: &ScreenshotConfig) -> ShotOutcome {
    if o == ShotOutcome::Accept {
        if args.pin {
            ShotOutcome::Pin
        } else if args.path.is_some() || args.clipboard || args.raw || args.print_geometry {
            ShotOutcome::Accept
        } else {
            ShotOutcome::from_raw(&cfg.return_action).unwrap_or(ShotOutcome::Copy)
        }
    } else {
        o
    }
}

// ---------------------------------------------------------------------------
// Key events (pure)
// ---------------------------------------------------------------------------

pub use crate::ui::popup::KeyInput;

const KEY_ESC: u16 = 53;
const KEY_RETURN: u16 = 36;
const KEY_ENTER: u16 = 76;
const KEY_DELETE: u16 = 51;
const KEY_FORWARD_DELETE: u16 = 117;
const KEY_TAB: u16 = 48;
const KEY_SPACE: u16 = 49;

/// `ScreenshotController.keyEvent(_:window:)` without the AppKit event.
pub fn shot_key_event(spec: &str) -> Option<KeyInput> {
    let mut mods = (false, false, false, false); // cmd, ctrl, shift, opt
    let mut key = String::new();
    for part in spec.to_lowercase().split('+') {
        match part {
            "cmd" => mods.0 = true,
            "ctrl" => mods.1 = true,
            "shift" => mods.2 = true,
            "opt" | "alt" => mods.3 = true,
            other => key = other.to_string(),
        }
    }
    let named: [(&str, u16); 9] = [
        ("esc", KEY_ESC),
        ("return", KEY_RETURN),
        ("delete", KEY_DELETE),
        ("space", KEY_SPACE),
        ("left", 123),
        ("right", 124),
        ("down", 125),
        ("up", 126),
        ("/", 44),
    ];
    let letters: [(char, u16); 26] = [
        ('a', 0),
        ('s', 1),
        ('d', 2),
        ('f', 3),
        ('h', 4),
        ('g', 5),
        ('z', 6),
        ('x', 7),
        ('c', 8),
        ('v', 9),
        ('b', 11),
        ('q', 12),
        ('w', 13),
        ('e', 14),
        ('r', 15),
        ('y', 16),
        ('t', 17),
        ('o', 31),
        ('u', 32),
        ('i', 34),
        ('p', 35),
        ('l', 37),
        ('j', 38),
        ('k', 40),
        ('n', 45),
        ('m', 46),
    ];
    let (code, chars) = if let Some((_, code)) = named.iter().find(|(n, _)| *n == key.as_str()) {
        (*code, key.chars().next())
    } else if key.chars().count() == 1 {
        let c = key.chars().next()?;
        let (_, code) = letters.iter().find(|(l, _)| *l == c)?;
        (*code, Some(c))
    } else {
        return None;
    };
    Some(KeyInput {
        key_code: code,
        chars,
        cmd: mods.0,
        ctrl: mods.1,
        shift: mods.2,
        opt: mods.3,
        esc_streak: 0,
    })
}

/// The window a screenshot keyDown targeted, mirroring the Swift monitor's
/// `e.window is ShotOverlayPanel` / `e.window as? PinPanel` checks without
/// AppKit. `Pin` carries the index into `ScreenshotController.pins`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShotKeyTarget {
    Overlay,
    Pin(usize),
}

/// Resolve which handler a keyDown belongs to from the event window's identity
/// (the pure half of `ScreenshotController.installKeyMonitor`'s block):
/// `is_overlay_window` is the `ShotOverlayPanel` downcast, `window_number` the
/// event window's `windowNumber()`, and `pin_window_numbers` parallels
/// `ScreenshotController.pins` (`PinPanel.window_number`; 0 = no live window
/// yet and never matches).
pub fn shot_key_target(
    is_overlay_window: bool,
    window_number: i64,
    pin_window_numbers: &[i64],
) -> Option<ShotKeyTarget> {
    if is_overlay_window {
        return Some(ShotKeyTarget::Overlay);
    }
    if window_number != 0 {
        if let Some(i) = pin_window_numbers.iter().position(|n| *n == window_number) {
            return Some(ShotKeyTarget::Pin(i));
        }
    }
    None
}

/// The [`KeyInput`] an `NSEvent` keyDown carries, for the overlay monitor
/// (mirrors the fields `ScreenshotOverlay.handleKey` reads).
#[cfg(target_os = "macos")]
pub fn key_input_from_event(event: &objc2_app_kit::NSEvent) -> KeyInput {
    use objc2_app_kit::NSEventModifierFlags as M;
    let mods = event.modifierFlags() & M::DeviceIndependentFlagsMask;
    KeyInput {
        key_code: event.keyCode(),
        chars: event
            .charactersIgnoringModifiers()
            .and_then(|s| s.to_string().chars().next()),
        cmd: mods.contains(M::Command),
        ctrl: mods.contains(M::Control),
        opt: mods.contains(M::Option),
        shift: mods.contains(M::Shift),
        esc_streak: 0,
    }
}

/// The [`PinKey`] an `NSEvent` keyDown carries, for the pin monitor (mirrors
/// the fields `PinPanel.handleKey` reads).
#[cfg(target_os = "macos")]
pub fn pin_key_from_event(event: &objc2_app_kit::NSEvent) -> PinKey {
    use objc2_app_kit::NSEventModifierFlags as M;
    let mods = event.modifierFlags() & M::DeviceIndependentFlagsMask;
    PinKey {
        key_code: event.keyCode(),
        ch: event
            .charactersIgnoringModifiers()
            .and_then(|s| s.to_string().chars().next()),
        cmd: mods.contains(M::Command),
    }
}

// __NEXT__

// ---------------------------------------------------------------------------
// ShotSession — overlay state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ShotDisplay {
    pub id: u32,
    pub frame: Rect,
    pub scale: f64,
    pub key: bool,
    pub level: i64,
    /// The live overlay panel's window number (0 until presented).
    pub wid: i64,
}

impl ShotDisplay {
    pub fn new(id: u32, frame: Rect, scale: f64) -> Self {
        ShotDisplay {
            id,
            frame,
            scale,
            key: false,
            wid: 0,
            // NSScreenSaverWindowLevel.
            level: 1000,
        }
    }
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.frame.w, self.frame.h)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEditState {
    pub index: Option<usize>,
    pub display: usize,
    pub object: ShotObject,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WheelState {
    pub display: usize,
    pub center: Point,
    pub hot: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveCardState {
    pub display: usize,
    pub field: String,
}

#[derive(Debug)]
enum Drag {
    None,
    Selecting {
        start: Point,
    },
    Resizing {
        handle: i64,
        from: Rect,
        start: Point,
    },
    MovingSelection {
        from: Rect,
        start: Point,
    },
    Drawing(ShotObject),
    MovingObject {
        index: usize,
        from: ShotObject,
        start: Point,
        moved: bool,
    },
}

pub struct ShotSession {
    pub cfg: ScreenshotConfig,
    pub args: ShotArgs,
    pub state: ShotState,
    pub state_path: String,
    pub doc: ShotDocument,
    pub displays: Vec<ShotDisplay>,
    /// Frozen capture per display index (parallel to `displays`); drives `render`.
    pub base_images: Vec<Option<BaseImage>>,
    pub active: Option<usize>,
    pub selection: Option<Rect>,
    pub tool: Option<ShotTool>,
    pub move_mode: bool,
    pub text_mode: bool,
    pub color: ShotColor,
    pub counter_offset: i64,
    pub side_panel_open: bool,
    pub grabbing: Option<ShotColor>,
    pub editing: Option<TextEditState>,
    pub wheel: Option<WheelState>,
    pub shortcuts_shown: bool,
    pub save_card: Option<SaveCardState>,
    pub chosen_save_path: Option<String>,
    pub finished: bool,
    pub outcome: Option<ShotOutcome>,
    pub recent_requested: bool,
    pub mouse: Option<(usize, Point)>,
    pub primary_max_y: f64,
    ring_new: bool,
    last_wheel_ms: i64,
    drag: Drag,
}

impl ShotSession {
    pub fn new(cfg: ScreenshotConfig, args: ShotArgs, state_path: impl Into<String>) -> Self {
        let state_path = state_path.into();
        let state = ShotState::load(&state_path);
        let color = state
            .color
            .as_deref()
            .and_then(ShotColor::from_hex)
            .unwrap_or(cfg.draw_color);
        let mut s = ShotSession {
            doc: ShotDocument::new(cfg.undo_limit.max(1) as usize),
            color,
            text_mode: args.mode == ShotMode::Text
                || (args.mode == ShotMode::Gui && cfg.start_text),
            cfg,
            args,
            state,
            state_path,
            displays: Vec::new(),
            base_images: Vec::new(),
            active: None,
            selection: None,
            tool: None,
            move_mode: false,
            counter_offset: 0,
            side_panel_open: false,
            grabbing: None,
            editing: None,
            wheel: None,
            shortcuts_shown: false,
            save_card: None,
            chosen_save_path: None,
            finished: false,
            outcome: None,
            recent_requested: false,
            mouse: None,
            primary_max_y: 0.0,
            ring_new: true,
            last_wheel_ms: 0,
            drag: Drag::None,
        };
        if s.state.style.family.is_empty() {
            s.state.style.family = s.cfg.font.clone();
        }
        s
    }

    pub fn add_display(&mut self, d: ShotDisplay) {
        if self.displays.is_empty() {
            self.primary_max_y = d.frame.max_y();
        }
        self.displays.push(d);
        self.base_images.push(None);
    }

    /// Attach the frozen capture for display index `i` (used by `render`).
    pub fn set_base_image(&mut self, i: usize, image: Option<BaseImage>) {
        if self.base_images.len() < self.displays.len() {
            self.base_images.resize(self.displays.len(), None);
        }
        if let Some(slot) = self.base_images.get_mut(i) {
            *slot = image;
        }
    }

    pub fn save_state(&self) {
        self.state.save(&self.state_path);
    }

    pub fn size(&self, t: ShotTool) -> i64 {
        self.state.size(t)
    }
    pub fn active_size(&self) -> Option<i64> {
        self.tool.map(|t| self.size(t))
    }
    pub fn current_color(&self) -> ShotColor {
        match self.doc.selected {
            Some(i) if i < self.doc.objects.len() => self.doc.objects[i].color,
            _ => self.color,
        }
    }
    pub fn mouse_display(&self) -> Option<usize> {
        self.mouse
            .map(|(d, _)| d)
            .or_else(|| if self.displays.is_empty() { None } else { Some(0) })
    }
    pub fn active_display(&self) -> Option<&ShotDisplay> {
        self.active.and_then(|i| self.displays.get(i))
    }
    pub fn global_origin(&self, d: &ShotDisplay) -> Point {
        Point::new(d.frame.x, self.primary_max_y - d.frame.max_y())
    }
    pub fn global_selection(&self) -> Option<Rect> {
        let d = self.active_display()?;
        let s = self.selection?;
        let o = self.global_origin(d);
        Some(Rect::new(s.x + o.x, s.y + o.y, s.w, s.h))
    }
    pub fn drawing(&self) -> Option<ShotObject> {
        if let Drag::Drawing(o) = &self.drag {
            Some(o.clone())
        } else {
            None
        }
    }
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }
    pub fn take_ring_new(&mut self) -> bool {
        let v = self.ring_new;
        self.ring_new = false;
        v
    }

    // -- tool / mode ------------------------------------------------------

    pub fn set_tool(&mut self, t: Option<ShotTool>) {
        self.commit_text();
        self.move_mode = false;
        self.tool = if t == self.tool { None } else { t };
        self.doc.selected = None;
    }
    pub fn force_tool(&mut self, t: Option<ShotTool>) {
        if self.tool != t {
            self.set_tool(t);
        }
    }
    pub fn toggle_text_mode(&mut self) {
        self.commit_text();
        self.close_wheel();
        if let Some(prev) = self.grabbing {
            self.color = prev;
            self.grabbing = None;
        }
        if self.side_panel_open {
            self.toggle_side_panel(Some(false));
        }
        self.shortcuts_shown = false;
        self.text_mode = !self.text_mode;
        self.tool = None;
        self.move_mode = false;
        self.doc.selected = None;
    }
    pub fn toggle_move_mode(&mut self) {
        self.commit_text();
        self.move_mode = !self.move_mode;
        if self.move_mode {
            self.tool = None;
        }
    }
    pub fn set_color(&mut self, c: ShotColor) {
        if let Some(i) = self.doc.selected {
            if i < self.doc.objects.len() {
                self.doc.update(i, None, |o| o.color = c);
            }
        } else {
            self.color = c;
            self.state.color = Some(c.hex());
            self.save_state();
        }
    }
    pub fn change_size(&mut self, delta: i64) {
        if let Some(i) = self.doc.selected {
            if i < self.doc.objects.len() {
                let t = self.doc.objects[i].tool;
                let (lo, hi) = t.size_range();
                self.doc.update(i, Some(&format!("size{i}")), |o| {
                    o.size = (o.size + delta).clamp(lo, hi)
                });
                return;
            }
        }
        let Some(t) = self.tool else { return };
        let (lo, hi) = t.size_range();
        let v = (self.size(t) + delta).clamp(lo, hi);
        self.state.sizes.insert(t.raw_value().to_string(), v);
        self.save_state();
    }
    pub fn set_size(&mut self, v: i64) {
        let cur = match self.doc.selected {
            Some(i) if i < self.doc.objects.len() => Some(self.doc.objects[i].size),
            _ => self.tool.map(|t| self.size(t)),
        };
        if let Some(cur) = cur {
            self.change_size(v - cur);
        }
    }

    // -- selection --------------------------------------------------------

    pub fn new_object(&self, t: ShotTool, p: Point) -> ShotObject {
        let mut o = ShotObject::new(t, vec![p, p], self.color, self.size(t));
        o.open_arrow = self.cfg.arrow_style == 1;
        o.reversed = self.cfg.reverse_arrow;
        o.outline = self.cfg.counter_outline;
        o.secure = !self.cfg.insecure_pixelate;
        o.style = self.state.style.clone();
        if t == ShotTool::Counter {
            o.points = vec![p];
            o.number_offset = self.counter_offset;
        }
        if t == ShotTool::Pencil {
            o.points = vec![p];
        }
        o
    }

    pub fn select(&mut self, r: Option<Rect>, on: Option<usize>, new: bool) {
        if let Some(d) = on {
            if Some(d) != self.active {
                self.doc.reset();
                self.active = Some(d);
            }
        }
        if r.is_none() {
            self.active = on.or(self.active);
        }
        self.selection = r.and_then(|x| {
            let std = standardized(x);
            let bounds = self.active_display().map(|d| d.bounds()).unwrap_or(x);
            std.intersection(&bounds)
                .filter(|s| s.w >= 0.5 && s.h >= 0.5)
        });
        if new {
            self.ring_new = true;
        }
    }

    pub fn select_all(&mut self) {
        if let Some(d) = self.mouse_display() {
            let bounds = self.displays[d].bounds();
            self.select(Some(bounds), Some(d), true);
        }
    }

    pub fn test_select(&mut self, r: Rect, on: Option<usize>) {
        let Some(d) = on.or(self.mouse_display()) else {
            return;
        };
        self.select(Some(r), Some(d), true);
    }

    pub fn test_draw(&mut self, a: Point, b: Point) {
        let Some(t) = self.tool else { return };
        let mut o = self.new_object(t, a);
        if t == ShotTool::Text {
            o.text = "text".to_string();
            o.points = vec![a];
        } else if t == ShotTool::Counter {
            o.points = if a == b { vec![a] } else { vec![a, b] };
        } else if t == ShotTool::Pencil {
            o.points = vec![a, Point::new((a.x + b.x) / 2.0, a.y), b];
        } else {
            o.points = vec![a, b];
        }
        self.doc.add(o);
        if t == ShotTool::Counter {
            self.counter_offset = 0;
        }
    }

    // -- text editing -----------------------------------------------------

    pub fn begin_text(&mut self, d: usize, at: Point, editing: Option<usize>) {
        self.commit_text();
        let mut o = match editing {
            Some(i) => self.doc.objects[i].clone(),
            None => self.new_object(ShotTool::Text, at),
        };
        if editing.is_none() {
            o.points = vec![at];
        }
        self.editing = Some(TextEditState {
            index: editing,
            display: d,
            object: o.clone(),
            text: o.text.clone(),
        });
        if editing.is_some() {
            self.doc.selected = None;
        }
    }

    pub fn commit_text(&mut self) {
        let Some(e) = self.editing.take() else { return };
        let text = e.text.trim_matches(|c| c == '\n' || c == '\r').to_string();
        let mut o = e.object;
        o.text = text.clone();
        match e.index {
            Some(i) => {
                if text.is_empty() {
                    self.doc.remove(i);
                } else {
                    self.doc.update(i, None, |x| *x = o);
                }
            }
            None => {
                if !text.is_empty() {
                    self.doc.add(o);
                }
            }
        }
    }

    pub fn set_text_style<F: FnMut(&mut ShotTextStyle)>(&mut self, mut f: F) {
        f(&mut self.state.style);
        self.save_state();
        if let Some(e) = &mut self.editing {
            f(&mut e.object.style);
        } else if let Some(i) = self.doc.selected {
            if i < self.doc.objects.len() && self.doc.objects[i].tool == ShotTool::Text {
                self.doc.update(i, None, |o| f(&mut o.style));
            }
        }
    }

    // -- side panel / shortcuts / grab / wheel ----------------------------

    pub fn toggle_side_panel(&mut self, open: Option<bool>) {
        let Some(d) = self.active.or(self.mouse_display()) else {
            return;
        };
        let _ = d;
        let want = open.unwrap_or(!self.side_panel_open);
        self.side_panel_open = want;
    }
    pub fn show_shortcuts(&mut self) {
        if self.mouse_display().is_none() {
            return;
        }
        self.shortcuts_shown = true;
    }
    pub fn hide_shortcuts(&mut self) {
        self.shortcuts_shown = false;
    }
    pub fn start_grab(&mut self) {
        self.commit_text();
        self.grabbing = Some(self.color);
    }
    pub fn cancel_grab(&mut self) {
        if let Some(prev) = self.grabbing {
            self.color = prev;
        }
        self.grabbing = None;
    }
    pub fn close_wheel(&mut self) {
        self.wheel = None;
    }

    // -- nudge / resize ---------------------------------------------------

    pub fn nudge(&mut self, dx: f64, dy: f64, resize: bool, symmetric: bool) {
        let Some(a) = self.active else { return };
        let Some(mut s) = self.selection else { return };
        let bounds = self.displays[a].bounds();
        if symmetric {
            s = s.inset(-dx, -dy);
        } else if resize {
            s.w = 1.0f64.max(s.w + dx);
            s.h = 1.0f64.max(s.h + dy);
        } else {
            s.x += dx;
            s.y += dy;
            s.x = 0.0f64.max(s.x.min(bounds.w - s.w));
            s.y = 0.0f64.max(s.y.min(bounds.h - s.h));
        }
        self.select(Some(s), Some(a), false);
    }

    pub fn resize(r: Rect, handle: i64, d: (f64, f64), mirror: bool, keep_aspect: bool) -> Rect {
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            (r.min_x(), r.min_y(), r.max_x(), r.max_y());
        let left = [0, 6, 7].contains(&handle);
        let right = [2, 3, 4].contains(&handle);
        let top = [0, 1, 2].contains(&handle);
        let bottom = [4, 5, 6].contains(&handle);
        let (mut dx, mut dy) = d;
        if keep_aspect && r.w > 0.0 && r.h > 0.0 && (left || right) && (top || bottom) {
            let ratio = r.w / r.h;
            let sx = if left { -dx } else { dx };
            let sy = if top { -dy } else { dy };
            let s = sx.max(sy * ratio);
            dx = if left { -s } else { s };
            dy = (if top { -1.0 } else { 1.0 }) * s / ratio;
        }
        if left {
            min_x += dx;
            if mirror {
                max_x -= dx;
            }
        }
        if right {
            max_x += dx;
            if mirror {
                min_x -= dx;
            }
        }
        if top {
            min_y += dy;
            if mirror {
                max_y -= dy;
            }
        }
        if bottom {
            max_y += dy;
            if mirror {
                min_y -= dy;
            }
        }
        ShotGeom::rect(Point::new(min_x, min_y), Point::new(max_x, max_y))
    }

    fn handle_points(s: Rect) -> [Point; 8] {
        selection_handles(s)
    }

    fn handle_at(&self, p: Point) -> Option<i64> {
        let s = self.selection?;
        selection_handle_at(s, p, self.cfg.button_size)
    }

    // -- key routing ------------------------------------------------------

    pub fn handle_key(&mut self, k: &KeyInput) -> bool {
        let ch = k.chars.map(|c| c.to_ascii_lowercase());
        let code = k.key_code;
        let is_esc = code == KEY_ESC;
        let text_responder = self.editing.is_some() || self.save_card.is_some();

        if text_responder {
            if is_esc {
                if self.save_card.is_some() {
                    self.close_save_card();
                } else if self.editing.is_some() {
                    self.commit_text();
                }
                return true;
            }
            if self.editing.is_some() && k.cmd && code == KEY_RETURN {
                self.commit_text();
                return true;
            }
            if k.cmd && ch == Some('q') {
                self.finish(ShotOutcome::Abort);
                return true;
            }
            return false;
        }

        if is_esc {
            self.escape();
            return true;
        }
        if self.shortcuts_shown {
            self.hide_shortcuts();
            return true;
        }
        if k.cmd && ch == Some('q') {
            self.finish(ShotOutcome::Abort);
            return true;
        }
        if k.cmd && ch == Some('/') {
            self.show_shortcuts();
            return true;
        }
        if k.cmd && ch == Some('r') {
            self.pressed(ShotTool::Recent);
            return true;
        }
        if code == KEY_TAB && !k.cmd && !k.ctrl && !k.opt {
            self.toggle_text_mode();
            return true;
        }
        if k.cmd && k.shift && ch == Some('c') {
            self.finish(ShotOutcome::Text);
            return true;
        }
        if self.text_mode {
            if (k.cmd || k.ctrl) && ch == Some('c') {
                self.finish(ShotOutcome::Text);
                return true;
            }
            if code == KEY_RETURN || code == KEY_ENTER {
                self.finish(ShotOutcome::Text);
                return true;
            }
            if k.cmd && ch == Some('a') {
                self.select_all();
                self.finish(ShotOutcome::Text);
                return true;
            }
            if !k.cmd && !k.ctrl && !k.opt && ch == Some('o') {
                self.toggle_text_mode();
            }
            return true;
        }
        if (k.cmd || k.ctrl) && ch == Some('c') && !k.shift {
            self.finish(ShotOutcome::Copy);
            return true;
        }
        if k.cmd && ch == Some('s') {
            self.request_save();
            return true;
        }
        if k.cmd && ch == Some('a') {
            self.select_all();
            return true;
        }
        if k.cmd && ch == Some('m') {
            self.toggle_move_mode();
            return true;
        }
        if k.cmd && ch == Some('z') {
            if k.shift {
                self.redo();
            } else {
                self.undo();
            }
            return true;
        }
        if k.cmd && code == KEY_RETURN {
            self.commit_text();
            return true;
        }
        if code == KEY_RETURN || code == KEY_ENTER {
            self.finish(ShotOutcome::Accept);
            return true;
        }
        if code == KEY_DELETE || code == KEY_FORWARD_DELETE {
            if let Some(i) = self.doc.selected {
                self.doc.remove(i);
            }
            return true;
        }
        let arrows: [(u16, f64, f64); 4] = [
            (123, -1.0, 0.0),
            (124, 1.0, 0.0),
            (125, 0.0, 1.0),
            (126, 0.0, -1.0),
        ];
        if let Some((_, dx, dy)) = arrows.iter().find(|(c, _, _)| *c == code) {
            if k.cmd && k.shift {
                self.nudge(*dx, *dy, true, true);
            } else if k.shift {
                self.nudge(*dx, *dy, true, false);
            } else if !k.cmd {
                self.nudge(*dx, *dy, false, false);
            }
            return true;
        }
        if !k.cmd && !k.ctrl && !k.opt {
            match ch {
                Some(' ') => {
                    self.toggle_side_panel(None);
                    return true;
                }
                Some('g') => {
                    self.start_grab();
                    return true;
                }
                Some('o') => {
                    self.toggle_text_mode();
                    return true;
                }
                _ => {}
            }
            if let Some(c) = ch {
                if let Some(t) = ShotTool::for_letter(c) {
                    self.set_tool(Some(t));
                    return true;
                }
            }
        }
        true
    }

    /// The Esc chain (Swift `ShotSession.escape`): save card → text → shortcuts
    /// → color wheel → grab → side panel → selected object → tool → close.
    pub fn escape(&mut self) {
        if self.save_card.is_some() {
            self.close_save_card();
            return;
        }
        if self.editing.is_some() {
            self.commit_text();
            return;
        }
        if self.shortcuts_shown {
            self.hide_shortcuts();
            return;
        }
        if self.wheel.is_some() {
            self.close_wheel();
            return;
        }
        if self.grabbing.is_some() {
            self.cancel_grab();
            return;
        }
        if self.side_panel_open {
            self.toggle_side_panel(Some(false));
            return;
        }
        if self.doc.selected.is_some() {
            self.doc.selected = None;
            return;
        }
        if self.tool.is_some() || self.move_mode {
            self.tool = None;
            self.move_mode = false;
            return;
        }
        self.finish(ShotOutcome::Abort);
    }

    pub fn undo(&mut self) {
        self.commit_text();
        self.doc.undo();
    }
    pub fn redo(&mut self) {
        self.commit_text();
        self.doc.redo();
    }

    pub fn pressed(&mut self, t: ShotTool) {
        if t.is_drawing() {
            self.set_tool(Some(t));
            return;
        }
        match t {
            ShotTool::Move => self.toggle_move_mode(),
            ShotTool::Undo => self.undo(),
            ShotTool::Redo => self.redo(),
            ShotTool::Copy => self.finish(ShotOutcome::Copy),
            ShotTool::Save => self.request_save(),
            ShotTool::Accept => self.finish(ShotOutcome::Accept),
            ShotTool::Exit => self.finish(ShotOutcome::Abort),
            ShotTool::Pin => self.finish(ShotOutcome::Pin),
            ShotTool::CopyText => self.finish(ShotOutcome::Text),
            ShotTool::Recent => self.recent_requested = true,
            ShotTool::SizeUp => self.change_size(1),
            ShotTool::SizeDown => self.change_size(-1),
            _ => {}
        }
    }

    pub fn request_save(&mut self) {
        if self.selection.is_none() {
            return;
        }
        if self.cfg.save_path_fixed || self.args.path.is_some() {
            self.finish(ShotOutcome::Save);
            return;
        }
        let Some(d) = self.active else { return };
        if self.save_card.is_some() {
            return;
        }
        self.commit_text();
        self.close_wheel();
        let dir = expand_tilde(&self.cfg.save_path);
        let path = ShotFiles::unique_path(
            &dir,
            &ShotFiles::expand(&self.cfg.filename_pattern),
            &self.cfg.save_format,
        );
        self.save_card = Some(SaveCardState {
            display: d,
            field: abbreviate_tilde(&path),
        });
    }

    pub fn confirm_save(&mut self, typed: &str) {
        if self.save_card.is_none() {
            return;
        }
        let p = expand_tilde(typed.trim());
        if p.is_empty() {
            return;
        }
        self.chosen_save_path = Some(p);
        self.close_save_card();
        self.finish(ShotOutcome::Save);
    }
    pub fn close_save_card(&mut self) {
        self.save_card = None;
    }

    pub fn finish(&mut self, o: ShotOutcome) {
        if self.finished {
            return;
        }
        self.commit_text();
        if o != ShotOutcome::Abort && o != ShotOutcome::Save && self.selection.is_none() {
            return;
        }
        if o == ShotOutcome::Save && self.selection.is_none() {
            return;
        }
        self.finished = true;
        self.save_state();
        self.outcome = Some(o);
    }

    // -- scroll / mouse (pure model; no AppKit invalidation) ---------------

    pub fn scroll(&mut self, delta_y: f64, precise: bool, cmd: bool) {
        if precise && (now_millis() - self.last_wheel_ms < 200 || delta_y.abs() <= 0.5) {
            return;
        }
        if delta_y == 0.0 {
            return;
        }
        self.last_wheel_ms = now_millis();
        let step = if delta_y > 0.0 { 1 } else { -1 };
        if cmd && self.tool == Some(ShotTool::Counter) {
            let next = self.doc.next_counter_number(0);
            self.counter_offset = (-next + 1).max(998i64.min(self.counter_offset + step));
            return;
        }
        self.change_size(step);
    }

    pub fn mouse_down(&mut self, d: usize, p: Point, click_count: i64, shift: bool, cmd: bool) {
        let _ = (shift, cmd);
        self.mouse = Some((d, p));
        if self.shortcuts_shown {
            self.hide_shortcuts();
            return;
        }
        if self.wheel.is_some() {
            self.close_wheel();
            return;
        }
        if self.grabbing.is_some() {
            self.grabbing = None;
            return;
        }
        if self.editing.is_some() {
            self.commit_text();
            return;
        }
        if self.text_mode {
            self.drag = Drag::Selecting { start: p };
            self.select(Some(Rect::new(p.x, p.y, 0.0, 0.0)), Some(d), true);
            return;
        }
        let in_active = Some(d) == self.active;
        if click_count == 2 && in_active {
            if let Some(s) = self.selection {
                if s.contains_point(p) {
                    if self.tool == Some(ShotTool::Text) || self.tool.is_none() {
                        if let Some(i) = self.doc.hit(p) {
                            if self.doc.objects[i].tool == ShotTool::Text {
                                let at = self.doc.objects[i].start();
                                self.begin_text(d, at, Some(i));
                                return;
                            }
                        }
                    }
                    if self.cfg.copy_on_double_click && self.tool.is_none() {
                        self.finish(ShotOutcome::Copy);
                        return;
                    }
                }
            }
        }
        if in_active {
            if let Some(h) = self.handle_at(p) {
                if let Some(from) = self.selection {
                    self.drag = Drag::Resizing {
                        handle: h,
                        from,
                        start: p,
                    };
                    return;
                }
            }
        }
        if in_active && self.move_mode {
            if let Some(from) = self.selection {
                self.drag = Drag::MovingSelection { from, start: p };
                return;
            }
        }
        if in_active && self.selection.is_some() {
            if let Some(i) = self.doc.hit(p) {
                self.doc.selected = Some(i);
                self.drag = Drag::MovingObject {
                    index: i,
                    from: self.doc.objects[i].clone(),
                    start: p,
                    moved: false,
                };
                return;
            }
        }
        self.doc.selected = None;
        if let Some(t) = self.tool {
            if in_active && self.selection.is_some() {
                if t == ShotTool::Text {
                    self.begin_text(d, p, None);
                    return;
                }
                self.drag = Drag::Drawing(self.new_object(t, p));
                return;
            }
        }
        if in_active && self.tool.is_none() {
            if let Some(s) = self.selection {
                if s.contains_point(p) {
                    self.drag = Drag::MovingSelection { from: s, start: p };
                    return;
                }
            }
        }
        self.drag = Drag::Selecting { start: p };
        self.select(Some(Rect::new(p.x, p.y, 0.0, 0.0)), Some(d), true);
    }

    pub fn mouse_dragged(&mut self, d: usize, p: Point, shift: bool, cmd: bool) {
        self.mouse = Some((d, p));
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::None => {}
            Drag::Selecting { start } => {
                let q = if shift { ShotSnap::square(start, p) } else { p };
                self.select(Some(ShotGeom::rect(start, q)), Some(d), true);
            }
            Drag::Resizing {
                handle,
                from,
                start,
            } => {
                let r =
                    ShotSession::resize(from, handle, (p.x - start.x, p.y - start.y), shift, cmd);
                self.select(Some(r), self.active, true);
            }
            Drag::MovingSelection { from, start } => {
                if let Some(a) = self.active {
                    let mut r = Rect::new(
                        from.x + (p.x - start.x),
                        from.y + (p.y - start.y),
                        from.w,
                        from.h,
                    );
                    let b = self.displays[a].bounds();
                    r.x = 0.0f64.max(r.x.min(b.w - r.w));
                    r.y = 0.0f64.max(r.y.min(b.h - r.h));
                    self.select(Some(r), Some(a), false);
                }
            }
            Drag::Drawing(mut o) => {
                match o.tool {
                    ShotTool::Pencil => {
                        if o.points
                            .last()
                            .map(|l| ShotGeom::dist(*l, p) >= 1.0)
                            .unwrap_or(true)
                        {
                            o.points.push(p);
                        }
                    }
                    ShotTool::Counter => o.points = vec![o.start(), p],
                    _ => {
                        o.points = vec![
                            o.start(),
                            if shift {
                                ShotSnap::snap(o.tool, o.start(), p)
                            } else {
                                p
                            },
                        ];
                    }
                }
                self.drag = Drag::Drawing(o);
            }
            Drag::MovingObject {
                index,
                from,
                start,
                ..
            } => {
                let dv = (p.x - start.x, p.y - start.y);
                self.doc.update_live(index, |o| *o = from.moved(dv.0, dv.1));
                self.drag = Drag::MovingObject {
                    index,
                    from,
                    start,
                    moved: true,
                };
            }
        }
    }

    pub fn mouse_up(&mut self, d: usize, p: Point, shift: bool, cmd: bool) {
        let _ = (d, p, shift, cmd);
        let was = std::mem::replace(&mut self.drag, Drag::None);
        match was {
            Drag::None => {}
            Drag::Selecting { .. } => {
                if let Some(s) = self.selection {
                    if s.w < 2.0 || s.h < 2.0 {
                        let active = self.active;
                        self.select(None, active, false);
                    }
                }
                if self.text_mode && self.selection.is_some() {
                    self.finish(ShotOutcome::Text);
                    return;
                }
                if self.args.accept_on_select && self.selection.is_some() {
                    self.finish(ShotOutcome::Accept);
                    return;
                }
            }
            Drag::Resizing { .. } | Drag::MovingSelection { .. } => {}
            Drag::Drawing(mut o) => {
                let r = o.rect();
                match o.tool {
                    ShotTool::Pencil | ShotTool::Counter => {
                        if o.tool == ShotTool::Counter
                            && o.points.len() > 1
                            && ShotGeom::dist(o.start(), o.end()) < o.counter_radius()
                        {
                            o.points = vec![o.start()];
                        }
                        let counter = o.tool == ShotTool::Counter;
                        self.doc.add(o);
                        if counter {
                            self.counter_offset = 0;
                        }
                    }
                    _ => {
                        if r.w.max(r.h) >= 2.0 {
                            self.doc.add(o);
                        }
                    }
                }
            }
            Drag::MovingObject {
                index,
                from,
                moved,
                ..
            } => {
                if moved {
                    let now = self.doc.objects[index].clone();
                    self.doc.update_live(index, |o| *o = from);
                    self.doc.update(index, None, |o| *o = now);
                    self.doc.selected = Some(index);
                }
            }
        }
    }

    /// `ShotRenderer.render`: crop the frozen image at the display's backing
    /// scale, then draw every object over it in top-left point space.
    pub fn render(&self) -> Option<ShotImage> {
        self.render_with(true)
    }

    /// `ShotRenderer.render(objects:)`. Swift's `finish` renders a text capture
    /// with `objects: false` so OCR reads the raw frozen image rather than the
    /// annotations drawn on top of it.
    pub fn render_with(&self, draw_objects: bool) -> Option<ShotImage> {
        let active = self.active?;
        let crop = self.selection?;
        let display = self.displays.get(active)?;
        #[cfg(target_os = "macos")]
        {
            let base = self.base_images.get(active).and_then(|b| b.as_deref())?;
            let scale = canvas_scale(
                objc2_core_graphics::CGImage::width(Some(base)) as i64,
                display.frame.w,
            );
            let empty: [ShotObject; 0] = [];
            let objects: &[ShotObject] = if draw_objects { &self.doc.objects } else { &empty };
            shot_cg::render_shot(base, scale, crop, objects)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (display, draw_objects);
            None
        }
    }
}

fn standardized(r: Rect) -> Rect {
    let x = if r.w < 0.0 { r.x + r.w } else { r.x };
    let y = if r.h < 0.0 { r.y + r.h } else { r.y };
    Rect::new(x, y, r.w.abs(), r.h.abs())
}

/// The eight resize handle centres for a selection (`ShotOverlayView.handlePoints`):
/// top-left, top-mid, top-right, right-mid, bottom-right, bottom-mid,
/// bottom-left, left-mid.
pub fn selection_handles(s: Rect) -> [Point; 8] {
    [
        Point::new(s.min_x(), s.min_y()),
        Point::new(s.mid_x(), s.min_y()),
        Point::new(s.max_x(), s.min_y()),
        Point::new(s.max_x(), s.mid_y()),
        Point::new(s.max_x(), s.max_y()),
        Point::new(s.mid_x(), s.max_y()),
        Point::new(s.min_x(), s.max_y()),
        Point::new(s.min_x(), s.mid_y()),
    ]
}

/// `ShotSession.handleAt(_:)`: the handle index under `p`, if any. The hit
/// radius matches the Swift `max(6, buttonSize * 0.3) + 2`.
pub fn selection_handle_at(sel: Rect, p: Point, button_size: f64) -> Option<i64> {
    let r = 6.0f64.max(button_size * 0.3) + 2.0;
    selection_handles(sel)
        .iter()
        .position(|h| ShotGeom::dist(p, *h) <= r)
        .map(|i| i as i64)
}

// ---------------------------------------------------------------------------
// Overlay chrome geometry (port of ScreenshotOverlay.swift's subview maths)
// ---------------------------------------------------------------------------
//
// The Rust overlay paints its chrome directly in `ShotOverlayView::render`
// instead of building AppKit subviews. These models port the pure layout each
// Swift subview computes (frame origins, hit-tests, content sizes) so the
// drawing pass and the socket tests share one source of truth.

/// `ShotWheelView` — the right-click colour wheel.
pub struct ShotWheel;

impl ShotWheel {
    pub const DOT: f64 = 26.0;

    /// `ShotWheelView.radius(_:)`.
    pub fn radius(n: usize) -> f64 {
        56.0f64.max(n as f64 * (Self::DOT + 8.0) / (2.0 * std::f64::consts::PI))
    }

    /// `ShotWheelView`'s swatch centre: index `i` starts at 12 o'clock and
    /// walks clockwise.
    pub fn dot_center(center: Point, i: usize, n: usize) -> Point {
        let a = -std::f64::consts::PI / 2.0
            + 2.0 * std::f64::consts::PI * i as f64 / n.max(1) as f64;
        Point::new(
            center.x + a.cos() * Self::radius(n),
            center.y + a.sin() * Self::radius(n),
        )
    }

    /// `ShotWheelView.index(at:center:count:)`; `None` for an empty wheel or a
    /// point that is not within `DOT/2 + 4` of a swatch.
    pub fn index_at(p: Point, center: Point, n: usize) -> Option<usize> {
        if n == 0 {
            return None;
        }
        (0..n).find(|&i| ShotGeom::dist(p, Self::dot_center(center, i, n)) <= Self::DOT / 2.0 + 4.0)
    }
}

/// `ShotLoupe` — the magnifier.
pub struct ShotLoupe;

impl ShotLoupe {
    pub const SIDE: f64 = 132.0;
    pub const PX: i64 = 15;

    /// The `ShotLoupe.update` origin: 24pt down-right of the pointer, flipped
    /// to the opposite side when the frame would overflow `bounds`.
    pub fn origin(p: Point, size: Size, bounds: Rect) -> Point {
        let mut o = Point::new(p.x + 24.0, p.y + 24.0);
        if o.x + size.w > bounds.max_x() {
            o.x = p.x - 24.0 - size.w;
        }
        if o.y + size.h > bounds.max_y() {
            o.y = p.y - 24.0 - size.h;
        }
        o
    }
}

/// `ShotRecentButton` — the recents button beside the mode pill.
pub struct ShotRecentButton;

impl ShotRecentButton {
    pub const SIZE: f64 = 28.0;

    /// `layoutChrome`'s recent-button origin: right of the mode pill and 3pt
    /// below its top edge (the pill starts at `top + 16`, the button at
    /// `top + 19`).
    pub fn origin(mode_pill: Rect) -> Point {
        Point::new(mode_pill.max_x() + 12.0, mode_pill.y + 3.0)
    }
}

/// `ShotModePill` — the Copy Text / Screenshot switch.
pub struct ShotModePill;

impl ShotModePill {
    pub const HEIGHT: f64 = 42.0;

    /// `layoutChrome`'s pill x: centred in the display.
    pub fn origin_x(bounds_width: f64, pill_width: f64) -> f64 {
        ((bounds_width - pill_width) / 2.0).round()
    }

    /// The pill's `mouseUp` segment hit test (`x >= 4 + widths[0]` picks the
    /// second segment).
    pub fn right_segment(x: f64, first_width: f64) -> bool {
        x >= 4.0 + first_width
    }
}

/// `ShotToolTab` — the vertical "Tool Settings" tab on the left edge.
pub struct ShotToolTab;

impl ShotToolTab {
    pub const WIDTH: f64 = 22.0;

    /// Centred vertically in the display.
    pub fn origin_y(bounds_height: f64, tab_height: f64) -> f64 {
        ((bounds_height - tab_height) / 2.0).round()
    }
}

/// `ShotSizeIndicator` — the transient tool-size readout.
pub struct ShotSizeIndicator;

impl ShotSizeIndicator {
    pub const FRAME: Rect = Rect {
        x: 20.0,
        y: 20.0,
        w: 56.0,
        h: 44.0,
    };
}

/// `ShotSaveCard` — the save-as card.
pub struct ShotSaveCard;

impl ShotSaveCard {
    pub const WIDTH: f64 = 520.0;
    pub const HEIGHT: f64 = 96.0;
    pub const TITLE_RECT: Rect = Rect {
        x: 16.0,
        y: 12.0,
        w: 300.0,
        h: 18.0,
    };
    pub const FIELD_RECT: Rect = Rect {
        x: 16.0,
        y: 36.0,
        w: 488.0,
        h: 24.0,
    };
    pub const HINT_RECT: Rect = Rect {
        x: 16.0,
        y: 68.0,
        w: 488.0,
        h: 16.0,
    };

    /// `ShotSaveCard.focus`: the (location, length) of the field selection —
    /// the file name minus its extension, as UTF-16 units (an `NSRange`).
    pub fn focus_range(path: &str) -> (usize, usize) {
        let name = path.rsplit('/').next().unwrap_or(path);
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        let path_len = path.encode_utf16().count();
        let name_len = name.encode_utf16().count();
        (path_len - name_len, stem.encode_utf16().count())
    }
}

/// `ShotHelpCard` — the centred key-hint card.
pub struct ShotHelpCard;

impl ShotHelpCard {
    /// The default 14pt system font line height (`ceil(ascender - descender + 6)`).
    pub const LINE_HEIGHT: f64 = 17.0;

    /// `ShotHelpCard` height: `ceil(lineHeight) * rows + 32`.
    pub fn height(rows: usize) -> f64 {
        Self::height_for(rows, Self::LINE_HEIGHT)
    }

    pub fn height_for(rows: usize, line_height: f64) -> f64 {
        line_height.ceil() * rows as f64 + 32.0
    }

    /// `layoutChrome`'s centred origin.
    pub fn centered_origin(bounds: Rect, size: Size) -> Point {
        Point::new(
            ((bounds.w - size.w) / 2.0).round(),
            ((bounds.h - size.h) / 2.0).round(),
        )
    }
}

/// `ShotSidePanel` — the tool-settings drawer.
pub struct ShotSidePanel;

impl ShotSidePanel {
    pub const WIDTH: f64 = 250.0;

    /// `build`'s layer list height: `max(80, bounds.height - y - 50)`.
    pub fn list_height(bounds_height: f64, y: f64) -> f64 {
        80.0f64.max(bounds_height - y - 50.0)
    }
}

fn expand_tilde(p: &str) -> String {
    if p == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| p.to_string());
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}

fn abbreviate_tilde(p: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            if let Some(rest) = p.strip_prefix(&home) {
                return format!("~{rest}");
            }
        }
    }
    p.to_string()
}

// __PART_F__

// ---------------------------------------------------------------------------
// ShotRenderer — the CoreGraphics/CoreText drawing pass (macOS only)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod shot_cg {
    use core::ffi::c_void;
    use core::ptr;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSImageCompressionFactor};
    use objc2_core_graphics::{
        kCGColorSpaceSRGB, CGAffineTransformMakeScale, CGBitmapContextCreate,
        CGBitmapContextCreateImage, CGBlendMode, CGColor, CGColorSpace, CGContext, CGImage,
        CGImageAlphaInfo, CGInterpolationQuality, CGLineCap, CGLineJoin, CGPath,
        CGPathDrawingMode,
    };
    use objc2_core_text::{
        kCTFontAttributeName, kCTForegroundColorAttributeName, CTFont, CTFontSymbolicTraits,
        CTFontUIFontType, CTLine,
    };
    use objc2_foundation::{
        NSData, NSDictionary, NSMutableAttributedString, NSNumber, NSPoint, NSRange, NSRect,
        NSSize, NSString,
    };

    use crate::engines::screenshot_annotations::{
        round_half_away, Point, Rect, ShotArrow, ShotColor, ShotGeom, ShotObject, ShotPixelate,
        ShotPixels, ShotText, ShotTextStyle, ShotTool, Size,
    };
    use super::{integral, output_size, px_rect, ShotImage};

    /// Toll-free reinterpret one opaque CF reference as another without naming
    /// the `objc2-core-foundation` type (mirrors `AnsiRender.swift`'s helper).
    fn ffi_cast<T, U>(r: &T) -> &U {
        unsafe { &*(r as *const T as *const U) }
    }

    fn cg_rect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
    }

    fn alloc_space() -> Option<Retained<CGColorSpace>> {
        let name = unsafe { kCGColorSpaceSRGB };
        CGColorSpace::with_name(Some(name)).map(|s| s.into())
    }

    fn bitmap_context(w: i64, h: i64) -> Option<Retained<CGContext>> {
        let space = alloc_space()?;
        unsafe {
            CGBitmapContextCreate(
                ptr::null_mut(),
                w.max(1) as usize,
                h.max(1) as usize,
                8,
                0,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .map(|c| c.into())
    }

    fn set_fill(ctx: &CGContext, c: ShotColor) {
        let color = CGColor::new_srgb(c.r, c.g, c.b, c.a);
        CGContext::set_fill_color_with_color(Some(ctx), Some(&*color));
    }

    fn set_stroke(ctx: &CGContext, c: ShotColor) {
        let color = CGColor::new_srgb(c.r, c.g, c.b, c.a);
        CGContext::set_stroke_color_with_color(Some(ctx), Some(&*color));
    }

    /// `ShotRenderer.drawImage`: place an image upright in the top-left space.
    pub(super) fn draw_image(ctx: &CGContext, img: &CGImage, r: Rect, smooth: bool) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        CGContext::save_g_state(Some(ctx));
        CGContext::set_interpolation_quality(
            Some(ctx),
            if smooth {
                CGInterpolationQuality::High
            } else {
                CGInterpolationQuality::None
            },
        );
        CGContext::translate_ctm(Some(ctx), r.x, r.max_y());
        CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
        CGContext::draw_image(
            Some(ctx),
            cg_rect(Rect::new(0.0, 0.0, r.w, r.h)),
            Some(img),
        );
        CGContext::restore_g_state(Some(ctx));
    }

    /// `ShotRenderer.drawBase`: crop the frozen image to `r` and place it.
    fn draw_base(ctx: &CGContext, base: &CGImage, scale: f64, size: Size, r: Rect) {
        let Some(r) = r.intersection(&Rect::new(0.0, 0.0, size.w, size.h)) else {
            return;
        };
        let px = px_rect(r, scale);
        let Some(crop) = CGImage::with_image_in_rect(Some(base), cg_rect(px)) else {
            return;
        };
        let cw = CGImage::width(Some(&crop)) as f64;
        let ch = CGImage::height(Some(&crop)) as f64;
        let pr = Rect::new(px.x / scale, px.y / scale, cw / scale, ch / scale);
        draw_image(ctx, &crop, pr, false);
    }

    /// `ShotCanvas.pixels`: rasterize the whole base image for the pixelate band.
    fn rasterize(base: &CGImage) -> Option<ShotPixels> {
        let w = CGImage::width(Some(base)) as i64;
        let h = CGImage::height(Some(base)) as i64;
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let space = alloc_space()?;
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr() as *mut c_void,
                w as usize,
                h as usize,
                8,
                (w * 4) as usize,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }?;
        CGContext::draw_image(
            Some(&*ctx),
            cg_rect(Rect::new(0.0, 0.0, w as f64, h as f64)),
            Some(base),
        );
        Some(ShotPixels::new(w, h, buf))
    }

    /// `ShotRenderer.stroke(_:_:width:)`.
    fn stroke_pts(ctx: &CGContext, pts: &[Point], w: f64) {
        let Some(f) = pts.first() else { return };
        if pts.len() == 1 {
            CGContext::fill_ellipse_in_rect(
                Some(ctx),
                cg_rect(Rect::new(f.x - w / 2.0, f.y - w / 2.0, w, w)),
            );
            return;
        }
        CGContext::begin_path(Some(ctx));
        CGContext::move_to_point(Some(ctx), f.x, f.y);
        for p in &pts[1..] {
            CGContext::add_line_to_point(Some(ctx), p.x, p.y);
        }
        CGContext::stroke_path(Some(ctx));
    }

    fn text_font(style: &ShotTextStyle, size: f64) -> Retained<CTFont> {
        let mut f: Retained<CTFont> = if style.family.is_empty() {
            unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::System, size, None) }
                .map(|f| f.into())
                .unwrap_or_else(|| {
                    let ns = NSString::from_str("Helvetica");
                    unsafe { CTFont::with_name(ffi_cast(&*ns), size, ptr::null()).into() }
                })
        } else {
            let ns = NSString::from_str(&style.family);
            unsafe { CTFont::with_name(ffi_cast(&*ns), size, ptr::null()).into() }
        };
        let mut traits = CTFontSymbolicTraits::empty();
        if style.bold {
            traits.insert(CTFontSymbolicTraits::TraitBold);
        }
        if style.italic {
            traits.insert(CTFontSymbolicTraits::TraitItalic);
        }
        if !traits.is_empty() {
            if let Some(t) =
                unsafe { f.copy_with_symbolic_traits(size, ptr::null(), traits, traits) }
            {
                f = t.into();
            }
        }
        f
    }

    fn text_line(text: &str, font: &CTFont, color: &CGColor) -> Retained<CTLine> {
        let ns = NSString::from_str(text);
        let attr =
            NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &ns);
        let raw = text.encode_utf16().count() as isize;
        let len = if raw == 0 { 0 } else { raw as usize };
        let font_obj: &AnyObject = ffi_cast(font);
        unsafe {
            attr.addAttribute_value_range(
                ffi_cast::<_, NSString>(kCTFontAttributeName),
                font_obj,
                NSRange::new(0, len),
            );
        }
        let color_obj: &AnyObject = ffi_cast(color);
        unsafe {
            attr.addAttribute_value_range(
                ffi_cast::<_, NSString>(kCTForegroundColorAttributeName),
                color_obj,
                NSRange::new(0, len),
            );
        }
        unsafe { CTLine::with_attributed_string(ffi_cast(&*attr)).into() }
    }

    fn text_metrics(font: &CTFont) -> (f64, f64) {
        let ascent = unsafe { font.ascent() };
        let descent = unsafe { font.descent() };
        let leading = unsafe { font.leading() };
        (ascent, (ascent + descent + leading).ceil())
    }

    fn line_width(line: &CTLine) -> f64 {
        unsafe { line.typographic_bounds(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) }
    }

    /// `(string as NSString).size(withAttributes:).width` for the overlay chrome
    /// (help-card columns, button badges), via the shared CoreText font path.
    pub(super) fn measure_text_width(text: &str, point_size: f64, bold: bool) -> f64 {
        let style = ShotTextStyle {
            family: String::new(),
            bold,
            italic: false,
            underline: false,
            strike: false,
            align: 0,
        };
        let font = text_font(&style, point_size);
        let color = CGColor::new_srgb(0.0, 0.0, 0.0, 1.0);
        line_width(&text_line(text, &font, &color))
    }

    fn text_box_size(lines: &[Retained<CTLine>], line_height: f64) -> Size {
        let mut max_w = line_height / 2.0;
        for l in lines {
            max_w = max_w.max(line_width(l));
        }
        Size::new(
            max_w.ceil() + ShotText::PADDING * 2.0,
            line_height * lines.len().max(1) as f64 + ShotText::PADDING * 2.0,
        )
    }

    pub(super) fn draw_text(ctx: &CGContext, o: &ShotObject) {
        let font = text_font(&o.style, o.font_size());
        let color = CGColor::new_srgb(o.color.r, o.color.g, o.color.b, o.color.a);
        let parts: Vec<&str> = o.text.split('\n').collect();
        let lines: Vec<Retained<CTLine>> = parts
            .iter()
            .map(|s| text_line(s, &font, &color))
            .collect();
        let (ascent, line_height) = text_metrics(&font);
        let box_size = text_box_size(&lines, line_height);
        let start = o.start();
        let inner = box_size.w - ShotText::PADDING * 2.0;
        // The context is y-down; a y-flipped text matrix keeps glyphs upright.
        CGContext::set_text_matrix(Some(ctx), CGAffineTransformMakeScale(1.0, -1.0));
        for (i, l) in lines.iter().enumerate() {
            let w = line_width(l);
            let dx = match o.style.align {
                1 => (inner - w) / 2.0,
                2 => inner - w,
                _ => 0.0,
            };
            let x = start.x + ShotText::PADDING + dx;
            let y = start.y + ShotText::PADDING + ascent + i as f64 * line_height;
            CGContext::set_text_position(Some(ctx), x, y);
            unsafe { l.draw(ctx) };
            if (o.style.underline || o.style.strike) && w > 0.0 {
                let t = 1.0f64.max(o.font_size() / 14.0);
                set_fill(ctx, o.color);
                if o.style.underline {
                    CGContext::fill_rect(Some(ctx), cg_rect(Rect::new(x, y + t * 2.0, w, t)));
                }
                if o.style.strike {
                    CGContext::fill_rect(
                        Some(ctx),
                        cg_rect(Rect::new(x, y - ascent * 0.32, w, t)),
                    );
                }
            }
        }
    }

    fn draw_counter(ctx: &CGContext, o: &ShotObject) {
        let c = o.start();
        let r = o.counter_radius();
        let contrast = if o.color.is_dark() {
            ShotColor::WHITE
        } else {
            ShotColor::BLACK
        };
        set_fill(ctx, o.color);
        if o.points.len() > 1 && ShotGeom::dist(c, o.end()) > r {
            let d = ShotGeom::dist(c, o.end());
            let ux = (o.end().x - c.x) / d;
            let uy = (o.end().y - c.y) / d;
            let half = r * 0.6;
            CGContext::begin_path(Some(ctx));
            CGContext::move_to_point(Some(ctx), c.x - uy * half, c.y + ux * half);
            CGContext::add_line_to_point(Some(ctx), o.end().x, o.end().y);
            CGContext::add_line_to_point(Some(ctx), c.x + uy * half, c.y - ux * half);
            CGContext::close_path(Some(ctx));
            CGContext::fill_path(Some(ctx));
        }
        let circle = Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r);
        CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(circle));
        if o.outline {
            set_stroke(ctx, contrast.with_alpha(0.9));
            CGContext::set_line_width(Some(ctx), 1.0);
            CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(circle.inset(0.5, 0.5)));
        }
        let mut style = ShotTextStyle::default();
        style.bold = true;
        let font = text_font(&style, r * if o.number > 99 { 0.75 } else { 1.0 });
        let label = format!("{}", o.number);
        let contrast_color = CGColor::new_srgb(contrast.r, contrast.g, contrast.b, contrast.a);
        let line = text_line(&label, &font, &contrast_color);
        let b = unsafe { line.image_bounds(None) };
        CGContext::set_text_matrix(Some(ctx), CGAffineTransformMakeScale(1.0, -1.0));
        let bm = b.mid();
        CGContext::set_text_position(Some(ctx), c.x - bm.x, c.y + bm.y);
        unsafe { line.draw(ctx) };
    }

    /// The insecure pixelate fallback: downscale the crop (a blur for size ≤ 1,
    /// a blocky pixelation grid for size > 1) — no CoreImage dependency.
    fn insecure_image(base: &CGImage, scale: f64, r: Rect, size: i64) -> Option<Retained<CGImage>> {
        let px = px_rect(r, scale);
        if px.w < 1.0 || px.h < 1.0 {
            return None;
        }
        let crop = CGImage::with_image_in_rect(Some(base), cg_rect(px))?;
        let (cols, rows) = if size <= 1 {
            let f = 1.0 / 20.0;
            (
                (1.0f64).max(round_half_away(px.w * f)),
                (1.0f64).max(round_half_away(px.h * f)),
            )
        } else {
            let (c, ro) = ShotPixelate::grid(r, size);
            (c as f64, ro as f64)
        };
        let ctx_owner = bitmap_context(cols as i64, rows as i64)?;
        let ctx: &CGContext = &ctx_owner;
        CGContext::set_interpolation_quality(
            Some(ctx),
            if size <= 1 {
                CGInterpolationQuality::High
            } else {
                CGInterpolationQuality::Medium
            },
        );
        CGContext::draw_image(
            Some(ctx),
            cg_rect(Rect::new(0.0, 0.0, cols, rows)),
            Some(&*crop),
        );
        CGBitmapContextCreateImage(Some(ctx)).map(|i| i.into())
    }

    fn draw_pixelate(
        ctx: &CGContext,
        base: &CGImage,
        scale: f64,
        pixels: &Option<ShotPixels>,
        o: &ShotObject,
    ) {
        let r = integral(o.rect());
        if r.w < 1.0 || r.h < 1.0 {
            return;
        }
        if o.secure {
            let Some(px) = pixels else { return };
            let blocks = ShotPixelate::secure_blocks(px, px_rect(r, scale), o.size, scale);
            let rows = blocks.len();
            let cols = blocks.first().map(|row| row.len()).unwrap_or(1);
            if rows == 0 || cols == 0 {
                return;
            }
            CGContext::set_should_antialias(Some(ctx), false);
            for (j, row) in blocks.iter().enumerate() {
                for (i, c) in row.iter().enumerate() {
                    let x0 = r.x + r.w * i as f64 / cols as f64;
                    let x1 = r.x + r.w * (i + 1) as f64 / cols as f64;
                    let y0 = r.y + r.h * j as f64 / rows as f64;
                    let y1 = r.y + r.h * (j + 1) as f64 / rows as f64;
                    set_fill(ctx, *c);
                    CGContext::fill_rect(Some(ctx), cg_rect(Rect::new(x0, y0, x1 - x0, y1 - y0)));
                }
            }
        } else if let Some(img) = insecure_image(base, scale, r, o.size) {
            draw_image(ctx, &img, r, o.size <= 1);
        }
    }

    fn draw_object(
        ctx: &CGContext,
        base: &CGImage,
        scale: f64,
        pixels: &Option<ShotPixels>,
        o: &ShotObject,
    ) {
        CGContext::save_g_state(Some(ctx));
        CGContext::set_should_antialias(Some(ctx), true);
        set_stroke(ctx, o.color);
        set_fill(ctx, o.color);
        CGContext::set_line_width(Some(ctx), o.stroke_width());
        CGContext::set_line_cap(Some(ctx), CGLineCap::Round);
        CGContext::set_line_join(Some(ctx), CGLineJoin::Round);
        match o.tool {
            ShotTool::Pencil => stroke_pts(ctx, &o.points, o.stroke_width()),
            ShotTool::Line => stroke_pts(ctx, &[o.start(), o.end()], 0.0),
            ShotTool::Marker => {
                CGContext::set_blend_mode(Some(ctx), CGBlendMode::Multiply);
                set_stroke(ctx, o.color.with_alpha(0.4));
                CGContext::set_line_cap(Some(ctx), CGLineCap::Butt);
                let pts: Vec<Point> = if o.points.len() > 2 {
                    o.points.clone()
                } else {
                    vec![o.start(), o.end()]
                };
                stroke_pts(ctx, &pts, o.stroke_width());
            }
            ShotTool::Arrow => {
                let (a, b) = if o.reversed {
                    (o.end(), o.start())
                } else {
                    (o.start(), o.end())
                };
                if ShotGeom::dist(a, b) <= 0.5 {
                    stroke_pts(ctx, &[a], o.stroke_width());
                } else {
                    let (tip, l, r, base_pt) = ShotArrow::head(a, b, o.size);
                    if o.open_arrow {
                        stroke_pts(ctx, &[a, b], 0.0);
                        stroke_pts(ctx, &[l, tip, r], 0.0);
                    } else {
                        stroke_pts(ctx, &[a, base_pt], 0.0);
                        CGContext::set_line_join(Some(ctx), CGLineJoin::Miter);
                        CGContext::set_line_width(Some(ctx), 1.0);
                        CGContext::begin_path(Some(ctx));
                        CGContext::move_to_point(Some(ctx), tip.x, tip.y);
                        CGContext::add_line_to_point(Some(ctx), l.x, l.y);
                        CGContext::add_line_to_point(Some(ctx), r.x, r.y);
                        CGContext::close_path(Some(ctx));
                        CGContext::draw_path(Some(ctx), CGPathDrawingMode::FillStroke);
                    }
                }
            }
            ShotTool::Selection => {
                CGContext::set_line_join(Some(ctx), CGLineJoin::Miter);
                CGContext::set_line_cap(Some(ctx), CGLineCap::Square);
                CGContext::stroke_rect(Some(ctx), cg_rect(o.rect()));
            }
            ShotTool::Rectangle => {
                let rect = o.rect();
                let rr = (o.size as f64).min(rect.w.min(rect.h) / 2.0);
                if rr > 0.0 {
                    let path =
                        unsafe { CGPath::with_rounded_rect(cg_rect(rect), rr, rr, ptr::null()) };
                    CGContext::add_path(Some(ctx), Some(&*path));
                } else {
                    CGContext::add_rect(Some(ctx), cg_rect(rect));
                }
                CGContext::fill_path(Some(ctx));
            }
            ShotTool::Circle => CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(o.rect())),
            ShotTool::Text => draw_text(ctx, o),
            ShotTool::Counter => draw_counter(ctx, o),
            ShotTool::Pixelate => draw_pixelate(ctx, base, scale, pixels, o),
            ShotTool::Invert => {
                CGContext::set_blend_mode(Some(ctx), CGBlendMode::Difference);
                set_fill(ctx, ShotColor::WHITE);
                CGContext::fill_rect(Some(ctx), cg_rect(o.rect()));
            }
            _ => {}
        }
        CGContext::restore_g_state(Some(ctx));
    }

    /// `ShotRenderer.render(_:crop:objects:)`, then PNG-encode the result via
    /// AppKit (the only non-CG step; ImageIO is not a dependency).
    pub fn render_shot(
        base: &CGImage,
        scale: f64,
        crop: Rect,
        objects: &[ShotObject],
    ) -> Option<ShotImage> {
        let bw = CGImage::width(Some(base));
        let bh = CGImage::height(Some(base));
        if bw == 0 || bh == 0 {
            return None;
        }
        let scale = scale.max(1.0);
        let size = Size::new(bw as f64 / scale, bh as f64 / scale);
        let crop = crop.intersection(&Rect::new(0.0, 0.0, size.w, size.h))?;
        if crop.w < 1.0 || crop.h < 1.0 {
            return None;
        }
        let (w, h) = output_size(crop, scale);
        if w <= 0 || h <= 0 {
            return None;
        }
        let ctx_owner = bitmap_context(w, h)?;
        let ctx: &CGContext = &ctx_owner;
        CGContext::translate_ctm(Some(ctx), 0.0, h as f64);
        CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
        CGContext::scale_ctm(Some(ctx), scale, scale);
        CGContext::translate_ctm(Some(ctx), -crop.x, -crop.y);
        CGContext::clip_to_rect(Some(ctx), cg_rect(crop));

        draw_base(ctx, base, scale, size, crop);

        let pixels = if objects.iter().any(|o| o.tool == ShotTool::Pixelate) {
            rasterize(base)
        } else {
            None
        };
        for o in objects {
            draw_object(ctx, base, scale, &pixels, o);
        }

        let image = CGBitmapContextCreateImage(Some(ctx))?;
        let png = encode_png(&image)?;
        Some(ShotImage::with_png(w, h, png))
    }

    pub(super) fn encode_png(image: &CGImage) -> Option<Vec<u8>> {
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
        let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> =
            NSDictionary::new();
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props)
        }?;
        Some(data.to_vec())
    }

    /// Re-encode an already-encoded PNG (a [`ShotImage`]'s `png`) as JPEG at
    /// `quality` (1…100), mirroring `ScreenshotController.write`'s
    /// `rep.representation(using: .jpeg, properties: [.compressionFactor: q])`.
    pub fn reencode_png_as_jpeg(png: &[u8], quality: i64) -> Option<Vec<u8>> {
        let data = NSData::with_bytes(png);
        let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)?;
        let q = (quality.clamp(1, 100) as f64) / 100.0;
        let key: &NSBitmapImageRepPropertyKey = unsafe { NSImageCompressionFactor };
        let value: Retained<AnyObject> = NSNumber::new_f64(q).into();
        let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> =
            NSDictionary::from_retained_objects(&[key], &[value]);
        let out = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &props)
        }?;
        Some(out.to_vec())
    }
}


// ---------------------------------------------------------------------------
// ShotOverlayPanel / ShotOverlayView — the live AppKit overlay (macOS only)
// ---------------------------------------------------------------------------
//
// One borderless, screen-level panel per display, sized to the display's
// bounds. Its content view shows the frozen capture and paints the dim mask
// outside the selection, the selection border and its eight resize handles,
// and the text draft while a text object is being edited. Mouse events feed
// the `ShotSession` selection/drawing state machine and the view invalidates
// itself afterwards. The controller owns the panels (see `OverlayPanels`) and
// orders them out on `close` / `end_session` / session replacement.
//
// There is no separate key monitor in `ui/popup.rs` for this overlay: the
// controller installs its own local keyDown monitor (`install_key_monitor`)
// while a session or pin is live, routing keys into `ShotSession::handle_key`
// / `PinPanel::handle_key`. Because the Rust daemon dispatches `do:` on the
// socket accept thread, panel creation (and monitor install) is guarded by
// `MainThreadMarker::new()` and simply no-ops when called off the main thread.

#[cfg(target_os = "macos")]
mod overlay {
    use std::cell::{Cell, RefCell};

    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags, NSGraphicsContext, NSPanel,
        NSScreenSaverWindowLevel, NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
        NSWindowStyleMask,
    };
    use objc2_core_graphics::{CGColor, CGContext, CGPathDrawingMode};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    use super::{
        selection_handles, shot_cg, ButtonRing, Point, Rect, ShotColor, ShotHelpCard, ShotLoupe,
        ShotObject, ShotSaveCard, ShotSession, ShotTool, ShotWheel,
    };
    use crate::engines::screenshot_annotations::ShotText;

    /// The blue the Swift overlay uses for text mode (`shotTextBlue`).
    const SHOT_TEXT_BLUE: ShotColor = ShotColor {
        r: 0x3a as f64 / 255.0,
        g: 0xa0 as f64 / 255.0,
        b: 0xff as f64 / 255.0,
        a: 1.0,
    };

    fn cg_rect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
    }

    fn set_fill(ctx: &CGContext, c: ShotColor) {
        let color = CGColor::new_srgb(c.r, c.g, c.b, c.a);
        CGContext::set_fill_color_with_color(Some(ctx), Some(&*color));
    }

    fn set_stroke(ctx: &CGContext, c: ShotColor) {
        let color = CGColor::new_srgb(c.r, c.g, c.b, c.a);
        CGContext::set_stroke_color_with_color(Some(ctx), Some(&*color));
    }

    fn fill_rect(ctx: &CGContext, r: Rect, c: ShotColor) {
        set_fill(ctx, c);
        CGContext::fill_rect(Some(ctx), cg_rect(r));
    }

    #[derive(Clone, Copy)]
    enum MousePhase {
        Down,
        Dragged,
        Up,
    }

    pub struct ShotOverlayViewIvars {
        /// The live session. Owned by the controller; cleared before it drops.
        session: Cell<*mut ShotSession>,
        display: Cell<usize>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShotOverlayView"]
        #[ivars = ShotOverlayViewIvars]
        pub struct ShotOverlayView;

        impl ShotOverlayView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.render();
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                self.feed(event, MousePhase::Down);
            }

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, event: &NSEvent) {
                self.feed(event, MousePhase::Dragged);
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, event: &NSEvent) {
                self.feed(event, MousePhase::Up);
            }

            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, event: &NSEvent) {
                self.feed_scroll(event);
            }
        }

        unsafe impl NSObjectProtocol for ShotOverlayView {}
    );

    impl ShotOverlayView {
        pub fn create(
            mtm: MainThreadMarker,
            display: usize,
            session: *mut ShotSession,
            size: NSSize,
        ) -> Retained<ShotOverlayView> {
            let this = ShotOverlayView::alloc(mtm).set_ivars(ShotOverlayViewIvars {
                session: Cell::new(session),
                display: Cell::new(display),
            });
            let view: Retained<ShotOverlayView> = unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)
                ]
            };
            view.setWantsLayer(true);
            view
        }

        fn session_ptr(&self) -> *mut ShotSession {
            self.ivars().session.get()
        }

        fn point(&self, event: &NSEvent) -> Point {
            let loc = event.locationInWindow();
            let p = self.convertPoint_fromView(loc, None);
            Point::new(p.x, p.y)
        }

        /// Route a mouse event into the session's selection state machine.
        fn feed(&self, event: &NSEvent, phase: MousePhase) {
            let ptr = self.session_ptr();
            if ptr.is_null() {
                return;
            }
            // SAFETY: `ptr` is a live `ShotSession` owned by the controller on
            // the main thread; the panels are ordered out and the pointer
            // cleared before the session is dropped.
            let session = unsafe { &mut *ptr };
            let d = self.ivars().display.get();
            if session.displays.get(d).is_none() {
                return;
            }
            let p = self.point(event);
            let click = event.clickCount() as i64;
            let mods = event.modifierFlags();
            let shift = mods.contains(NSEventModifierFlags::Shift);
            let cmd = mods.contains(NSEventModifierFlags::Command);
            match phase {
                MousePhase::Down => session.mouse_down(d, p, click, shift, cmd),
                MousePhase::Dragged => session.mouse_dragged(d, p, shift, cmd),
                MousePhase::Up => session.mouse_up(d, p, shift, cmd),
            }
            self.setNeedsDisplay(true);
        }

        /// `ShotSession.scroll`: the mouse wheel changes the active tool size.
        fn feed_scroll(&self, event: &NSEvent) {
            let ptr = self.session_ptr();
            if ptr.is_null() {
                return;
            }
            let session = unsafe { &mut *ptr };
            let d = self.ivars().display.get();
            if session.displays.get(d).is_none() {
                return;
            }
            session.mouse = Some((d, self.point(event)));
            let cmd = event.modifierFlags().contains(NSEventModifierFlags::Command);
            session.scroll(
                event.scrollingDeltaY() as f64,
                event.hasPreciseScrollingDeltas(),
                cmd,
            );
            self.setNeedsDisplay(true);
        }

        /// `ShotOverlayView.draw(_:)`: the frozen capture, the dim mask, the
        /// selection border + handles, the text draft, then the chrome
        /// subviews (help card, button ring, colour wheel, loupe, save card)
        /// via [`Self::render_chrome`].
        fn render(&self) {
            let ptr = self.session_ptr();
            if ptr.is_null() {
                return;
            }
            // SAFETY: same as `feed` — main-thread, live session, not cleared.
            let session = unsafe { &*ptr };
            let d = self.ivars().display.get();
            let Some(_display) = session.displays.get(d) else {
                return;
            };
            let Some(gc) = NSGraphicsContext::currentContext() else {
                return;
            };
            let ctx = gc.CGContext();
            let bounds = self.bounds();
            let w = bounds.size.width;
            let h = bounds.size.height;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let full = Rect::new(0.0, 0.0, w, h);
            let cfg = &session.cfg;

            // 1. The frozen capture, placed upright.
            match session.base_images.get(d).and_then(|b| b.as_deref()) {
                Some(img) => shot_cg::draw_image(&ctx, img, full, true),
                None => fill_rect(&ctx, full, ShotColor::BLACK),
            }

            // 2. Dim outside the selection (even-odd bounds + selection) for
            //    the active display; a plain fill when the display is not active.
            let mine = if session.active == Some(d) {
                session.selection
            } else {
                None
            };
            let dim = cfg
                .contrast_color
                .with_alpha(cfg.contrast_opacity as f64 / 255.0);
            match mine {
                Some(sel) => {
                    set_fill(&ctx, dim);
                    CGContext::save_g_state(Some(&ctx));
                    CGContext::begin_path(Some(&ctx));
                    CGContext::add_rect(Some(&ctx), cg_rect(full));
                    CGContext::add_rect(Some(&ctx), cg_rect(sel));
                    CGContext::draw_path(Some(&ctx), CGPathDrawingMode::EOFill);
                    CGContext::restore_g_state(Some(&ctx));
                }
                None => fill_rect(&ctx, full, dim),
            }

            let mode_color = if session.text_mode {
                SHOT_TEXT_BLUE
            } else {
                cfg.ui_color
            };

            // 3. Selection border + the eight resize handles.
            if let Some(sel) = mine {
                if sel.w > 0.0 && sel.h > 0.0 {
                    set_stroke(&ctx, mode_color);
                    CGContext::set_line_width(Some(&ctx), 1.0);
                    CGContext::stroke_rect(
                        Some(&ctx),
                        cg_rect(Rect::new(sel.x - 0.5, sel.y - 0.5, sel.w + 1.0, sel.h + 1.0)),
                    );
                    if !session.is_dragging() {
                        let hd = (cfg.button_size * 0.6 * 0.5).round().max(2.0);
                        set_fill(&ctx, mode_color);
                        for p in selection_handles(sel) {
                            CGContext::fill_ellipse_in_rect(
                                Some(&ctx),
                                cg_rect(Rect::new(p.x - hd / 2.0, p.y - hd / 2.0, hd, hd)),
                            );
                        }
                    }
                }
            }

            // 4. The text draft while a text object is being edited (the one
            //    annotation the model exposes without the full renderer).
            if let Some(e) = &session.editing {
                if e.display == d {
                    shot_cg::draw_text(&ctx, &e.object);
                }
            }

            // 5. The chrome subviews (help card, button ring, colour wheel,
            //    loupe, save card), painted into this context.
            self.render_chrome(session, &ctx, full, d);
        }

        /// The overlay chrome subviews (`ScreenshotOverlay.swift`'s
        /// `ShotHelpCard` / `ShotButton` ring / `ShotWheelView` / `ShotLoupe` /
        /// `ShotSaveCard`), painted into the same context instead of as AppKit
        /// subviews. Layout comes from the pure models in the parent module;
        /// the side-panel form and the recent button / mode pill stay
        /// model-only (they are AppKit controls).
        fn render_chrome(&self, session: &ShotSession, ctx: &CGContext, full: Rect, d: usize) {
            let cfg = &session.cfg;
            let mine = if session.active == Some(d) {
                session.selection
            } else {
                None
            };

            // Help card: centred while nothing is selected.
            if cfg.show_help && session.selection.is_none() {
                draw_help_card(ctx, session, full);
            }

            // Button ring: only around a live selection, not in text mode.
            if let Some(sel) = mine {
                if !session.text_mode {
                    draw_button_ring(ctx, session, d, sel);
                }
            }

            // Colour wheel (right-click) on its display.
            if let Some(w) = &session.wheel {
                if w.display == d {
                    draw_wheel(ctx, session, w.center, w.hot);
                }
            }

            // Loupe while grabbing or magnifying with a hovered pointer.
            let want_loupe =
                session.grabbing.is_some() || (cfg.magnifier && session.selection.is_none());
            if want_loupe && session.mouse.map(|(md, _)| md) == Some(d) {
                if let Some((_, p)) = session.mouse {
                    draw_loupe(ctx, session, full, d, p);
                }
            }

            // Save card (the path field is not an editable AppKit field here,
            // but the card and its value paint).
            if let Some(card) = &session.save_card {
                if card.display == d {
                    draw_save_card(ctx, session, full, &card.field);
                }
            }
        }
    }

    /// Draw one line of chrome text with its top-left at `(x, y)`, reusing the
    /// shared CoreText text pass (a synthetic text `ShotObject`).
    fn draw_text_at(
        ctx: &CGContext,
        text: &str,
        x: f64,
        y: f64,
        point_size: f64,
        color: ShotColor,
        bold: bool,
    ) {
        let size = (point_size - 8.0).round().max(0.0) as i64;
        let mut o = ShotObject::new(
            ShotTool::Text,
            vec![Point::new(x - ShotText::PADDING, y - ShotText::PADDING)],
            color,
            size,
        );
        o.text = text.to_string();
        o.style.bold = bold;
        shot_cg::draw_text(ctx, &o);
    }

    /// `ShotHelpCard`: a centred rounded card of key/action rows.
    fn draw_help_card(ctx: &CGContext, session: &ShotSession, full: Rect) {
        let cfg = &session.cfg;
        let rows: &[(String, String)] = if session.text_mode {
            &cfg.help_text_rows
        } else {
            &cfg.help_rows
        };
        if rows.is_empty() {
            return;
        }
        let ui = if session.text_mode {
            SHOT_TEXT_BLUE
        } else {
            cfg.ui_color
        };
        let (point, bold) = (14.0, 14.0);
        let key_w = rows
            .iter()
            .map(|(k, _)| shot_cg::measure_text_width(k, bold, true))
            .fold(0.0f64, f64::max);
        let act_w = rows
            .iter()
            .map(|(_, a)| shot_cg::measure_text_width(a, point, false))
            .fold(0.0f64, f64::max);
        let size = super::Size::new(
            (key_w + act_w + 18.0 + 48.0).ceil(),
            ShotHelpCard::height(rows.len()),
        );
        let o = ShotHelpCard::centered_origin(full, size);
        let fg = if ui.is_dark() {
            ShotColor::WHITE
        } else {
            ShotColor::BLACK
        };
        set_fill(ctx, ui.with_alpha(0.92));
        CGContext::fill_rect(Some(ctx), cg_rect(Rect::new(o.x, o.y, size.w, size.h)));
        set_stroke(ctx, ShotColor::WHITE.with_alpha(0.45));
        CGContext::set_line_width(Some(ctx), 1.0);
        CGContext::stroke_rect(Some(ctx), cg_rect(Rect::new(o.x, o.y, size.w, size.h)));
        let mut y = o.y + 16.0;
        for (k, a) in rows {
            let kw = shot_cg::measure_text_width(k, bold, true);
            draw_text_at(ctx, k, o.x + 24.0 + key_w - kw, y, bold, fg, true);
            draw_text_at(ctx, a, o.x + 24.0 + key_w + 18.0, y, point, fg, false);
            y += ShotHelpCard::LINE_HEIGHT;
        }
    }

    /// `ShotButton`'s ring: one filled disc per tool at the `ButtonRing`
    /// frames, with the active tool highlighted and the size badge labelled.
    fn draw_button_ring(ctx: &CGContext, session: &ShotSession, d: usize, sel: Rect) {
        let cfg = &session.cfg;
        let Some(display) = session.displays.get(d) else {
            return;
        };
        let layout = ButtonRing::layout(sel, display.bounds(), cfg.buttons.len(), cfg.button_size);
        // A badged text tool uses dark-on-light; the button glyphs themselves
        // are SF Symbols in AppKit and are not redrawn here.
        for (t, f) in cfg.buttons.iter().zip(layout.frames.iter()) {
            let c = Point::new(f.mid_x(), f.mid_y());
            let r = (f.w / 2.0 - 0.5).max(1.0);
            let active = session.tool == Some(*t) || (*t == ShotTool::Move && session.move_mode);
            let fill = if active {
                cfg.ui_color.mixed(&ShotColor::BLACK, 0.25)
            } else {
                cfg.ui_color
            };
            set_fill(ctx, fill);
            CGContext::fill_ellipse_in_rect(
                Some(ctx),
                cg_rect(Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r)),
            );
            if active {
                set_stroke(ctx, cfg.contrast_color.mixed(&ShotColor::WHITE, 0.55));
                CGContext::set_line_width(Some(ctx), 2.0);
                let rr = (r - 1.0).max(1.0);
                CGContext::stroke_ellipse_in_rect(
                    Some(ctx),
                    cg_rect(Rect::new(c.x - rr, c.y - rr, 2.0 * rr, 2.0 * rr)),
                );
            }
            if *t == ShotTool::Size {
                let badge = format!("{}\n{}", sel.w.round() as i64, sel.h.round() as i64);
                let fg = if cfg.ui_color.is_dark() {
                    ShotColor::WHITE
                } else {
                    ShotColor::BLACK
                };
                for (i, line) in badge.split('\n').enumerate() {
                    let w = shot_cg::measure_text_width(line, 10.0, true);
                    draw_text_at(
                        ctx,
                        line,
                        c.x - w / 2.0,
                        c.y - 10.0 + i as f64 * 11.0,
                        10.0,
                        fg,
                        true,
                    );
                }
            }
        }
    }

    /// `ShotWheelView`: the colour swatches around the current colour hub.
    fn draw_wheel(ctx: &CGContext, session: &ShotSession, center: Point, hot: Option<i64>) {
        let cfg = &session.cfg;
        let colors = &cfg.user_colors;
        if colors.is_empty() {
            return;
        }
        let r = ShotWheel::radius(colors.len());
        let dot = ShotWheel::DOT;
        set_fill(ctx, ShotColor::BLACK.with_alpha(0.35));
        let outer = r + dot * 0.75;
        CGContext::fill_ellipse_in_rect(
            Some(ctx),
            cg_rect(Rect::new(
                center.x - outer,
                center.y - outer,
                2.0 * outer,
                2.0 * outer,
            )),
        );
        set_fill(ctx, session.current_color());
        CGContext::fill_ellipse_in_rect(
            Some(ctx),
            cg_rect(Rect::new(center.x - 16.0, center.y - 16.0, 32.0, 32.0)),
        );
        set_stroke(ctx, ShotColor::WHITE);
        CGContext::set_line_width(Some(ctx), 2.0);
        CGContext::stroke_ellipse_in_rect(
            Some(ctx),
            cg_rect(Rect::new(center.x - 16.0, center.y - 16.0, 32.0, 32.0)),
        );
        for (i, col) in colors.iter().enumerate() {
            let q = ShotWheel::dot_center(center, i, colors.len());
            let is_hot = hot == Some(i as i64);
            let dd = dot * (if is_hot { 1.3 } else { 1.0 });
            let rect = Rect::new(q.x - dd / 2.0, q.y - dd / 2.0, dd, dd);
            match col {
                Some(c) => {
                    set_fill(ctx, *c);
                    CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(rect));
                }
                None => {
                    // The HSV picker swatch (12 wedges); approximated as a
                    // colour fan is out of scope, so show the current colour.
                    set_fill(ctx, session.current_color());
                    CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(rect));
                }
            }
            set_stroke(
                ctx,
                if is_hot {
                    ShotColor::WHITE
                } else {
                    ShotColor::WHITE.with_alpha(0.6)
                },
            );
            CGContext::set_line_width(Some(ctx), if is_hot { 2.5 } else { 1.0 });
            CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(rect));
        }
    }

    /// `ShotLoupe`: the magnified crop of the frozen capture.
    fn draw_loupe(ctx: &CGContext, session: &ShotSession, full: Rect, d: usize, p: Point) {
        let side = ShotLoupe::SIDE;
        let size = super::Size::new(side, side + 22.0);
        let o = ShotLoupe::origin(p, size, full);
        let circle = Rect::new(o.x, o.y, side, side);
        let Some(base) = session.base_images.get(d).and_then(|b| b.as_deref()) else {
            return;
        };
        let scale = super::canvas_scale(
            objc2_core_graphics::CGImage::width(Some(base)) as i64,
            session.displays.get(d).map(|x| x.frame.w).unwrap_or(0.0),
        );
        let cx = p.x * scale;
        let cy = p.y * scale;
        let half = (ShotLoupe::PX as f64) / 2.0;
        if let Some(crop) = objc2_core_graphics::CGImage::with_image_in_rect(
            Some(base),
            cg_rect(Rect::new(cx - half, cy - half, ShotLoupe::PX as f64, ShotLoupe::PX as f64)),
        ) {
            set_fill(ctx, ShotColor::BLACK);
            CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(circle));
            shot_cg::draw_image(ctx, &crop, circle, false);
        }
        set_stroke(ctx, cfg_uicolor(session));
        CGContext::set_line_width(Some(ctx), 2.0);
        CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(circle));
    }

    fn cfg_uicolor(session: &ShotSession) -> ShotColor {
        session.cfg.ui_color
    }

    /// `ShotSaveCard`: the save-as card frame, title, value and hint.
    fn draw_save_card(ctx: &CGContext, session: &ShotSession, full: Rect, path: &str) {
        let cfg = &session.cfg;
        let size = super::Size::new(ShotSaveCard::WIDTH, ShotSaveCard::HEIGHT);
        let o = ShotHelpCard::centered_origin(full, size);
        let card = Rect::new(o.x, o.y, size.w, size.h);
        set_fill(ctx, ShotColor::new_a(0.08, 0.08, 0.08, 0.95));
        CGContext::fill_rect(Some(ctx), cg_rect(card));
        set_stroke(ctx, cfg.ui_color);
        CGContext::set_line_width(Some(ctx), 1.5);
        CGContext::stroke_rect(Some(ctx), cg_rect(card));
        let shift = |r: Rect| Rect::new(o.x + r.x, o.y + r.y, r.w, r.h);
        draw_text_at(
            ctx,
            "Save screenshot as",
            shift(ShotSaveCard::TITLE_RECT).x,
            shift(ShotSaveCard::TITLE_RECT).y,
            13.0,
            ShotColor::WHITE,
            true,
        );
        let field = shift(ShotSaveCard::FIELD_RECT);
        set_stroke(ctx, ShotColor::WHITE.with_alpha(0.7));
        CGContext::set_line_width(Some(ctx), 1.0);
        CGContext::stroke_rect(Some(ctx), cg_rect(field));
        draw_text_at(
            ctx,
            path,
            field.x + 4.0,
            field.y + 4.0,
            12.0,
            ShotColor::WHITE,
            false,
        );
        let hint = shift(ShotSaveCard::HINT_RECT);
        draw_text_at(
            ctx,
            "Return saves (.png / .jpg) \u{b7} Esc goes back",
            hint.x,
            hint.y,
            11.0,
            ShotColor::new_a(0.7, 0.7, 0.7, 1.0),
            false,
        );
    }

    pub struct ShotOverlayPanelIvars {
        overlay: RefCell<Option<Retained<ShotOverlayView>>>,
    }

    define_class!(
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShotOverlayPanel"]
        #[ivars = ShotOverlayPanelIvars]
        pub struct ShotOverlayPanel;

        impl ShotOverlayPanel {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool {
                true
            }

            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool {
                false
            }
        }

        unsafe impl NSObjectProtocol for ShotOverlayPanel {}
    );

    impl ShotOverlayPanel {
        /// One borderless screen-level panel over `frame` (the display's
        /// bounds) with a [`ShotOverlayView`] content view bound to display
        /// `display` and the live `session`.
        pub fn create(
            mtm: MainThreadMarker,
            frame: Rect,
            display: usize,
            session: *mut ShotSession,
        ) -> Retained<ShotOverlayPanel> {
            let rect = cg_rect(frame);
            let this = ShotOverlayPanel::alloc(mtm)
                .set_ivars(ShotOverlayPanelIvars { overlay: RefCell::new(None) });
            let panel: Retained<ShotOverlayPanel> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: rect,
                    styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            panel.setLevel(NSScreenSaverWindowLevel);
            panel.setOpaque(true);
            panel.setBackgroundColor(Some(&NSColor::blackColor()));
            panel.setHasShadow(false);
            panel.setHidesOnDeactivate(false);
            panel.setAcceptsMouseMovedEvents(true);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
            unsafe { panel.setReleasedWhenClosed(false) };
            let view = ShotOverlayView::create(mtm, display, session, rect.size);
            let base: &NSView = &view;
            panel.setContentView(Some(base));
            *panel.ivars().overlay.borrow_mut() = Some(view);
            panel
        }

        /// `orderFrontRegardless` — show without activating the app.
        pub fn present(&self) {
            self.orderFrontRegardless();
        }

        pub fn overlay(&self) -> Option<Retained<ShotOverlayView>> {
            self.ivars().overlay.borrow().clone()
        }

        /// Detach the session pointer (called before the session is dropped).
        pub fn clear_session(&self) {
            if let Some(v) = self.overlay() {
                v.ivars().session.set(std::ptr::null_mut());
            }
        }

        pub fn redraw(&self) {
            if let Some(v) = self.overlay() {
                v.setNeedsDisplay(true);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WSPinPanel / WSPinView — the live always-on-top pin window (macOS only)
// ---------------------------------------------------------------------------
//
// Mirrors Swift `PinPanel: NSPanel` / `PinView: NSView` (ScreenshotPin.swift):
// a borderless non-activating panel at `.floating` level whose content view
// draws the pinned capture inset by [`PIN_MARGIN`]. It joins all spaces, is
// ignored by window cycling and never hides on deactivate. The controller owns
// the panels (see [`PinWindows`]); the pin key handling itself lives in the
// pure [`PinPanel`] model and the controller key monitor.
//
// The window is created only on the main thread (`MainThreadMarker::new()`).
// The socket `do:` path runs on the accept thread and so falls back to the
// model-only pin (window number 0) — see the `HOST-SEAM` note in `pin()`.
#[cfg(target_os = "macos")]
mod pin_window {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSBitmapImageRep, NSColor, NSFloatingWindowLevel, NSGraphicsContext,
        NSPanel, NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_core_graphics::{CGContext, CGImage};
    use objc2_foundation::{NSData, NSObjectProtocol, NSPoint, NSRect, NSSize};

    use super::{shot_cg, Rect, PIN_MARGIN};

    /// Decode encoded PNG bytes (a [`super::ShotImage`]'s `png`) back to a
    /// `CGImage` for the pin window. Returns `None` for bytes that are not a
    /// decodable image (e.g. the synthetic test payloads).
    pub fn decode_png(png: &[u8]) -> Option<Retained<CGImage>> {
        let data = NSData::with_bytes(png);
        let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)?;
        rep.CGImage()
    }

    pub struct WSPinViewIvars {
        image: RefCell<Option<Retained<CGImage>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPinView"]
        #[ivars = WSPinViewIvars]
        pub struct WSPinView;

        impl WSPinView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                let image = self.ivars().image.borrow().clone();
                let Some(image) = image else { return };
                let Some(gc) = NSGraphicsContext::currentContext() else {
                    return;
                };
                let ctx: Retained<CGContext> = gc.CGContext();
                let b = self.bounds();
                let r = Rect::new(
                    b.origin.x + PIN_MARGIN,
                    b.origin.y + PIN_MARGIN,
                    (b.size.width - PIN_MARGIN * 2.0).max(0.0),
                    (b.size.height - PIN_MARGIN * 2.0).max(0.0),
                );
                shot_cg::draw_image(&ctx, &image, r, true);
            }
        }

        unsafe impl NSObjectProtocol for WSPinView {}
    );

    impl WSPinView {
        fn create(
            mtm: MainThreadMarker,
            image: Retained<CGImage>,
            size: NSSize,
        ) -> Retained<WSPinView> {
            let this = WSPinView::alloc(mtm).set_ivars(WSPinViewIvars {
                image: RefCell::new(Some(image)),
            });
            let view: Retained<WSPinView> = unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)
                ]
            };
            view.setWantsLayer(true);
            view
        }
    }

    pub struct WSPinPanelIvars {
        view: RefCell<Option<Retained<WSPinView>>>,
    }

    define_class!(
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPinPanel"]
        #[ivars = WSPinPanelIvars]
        pub struct WSPinPanel;

        impl WSPinPanel {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool {
                true
            }

            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool {
                false
            }
        }

        unsafe impl NSObjectProtocol for WSPinPanel {}
    );

    impl WSPinPanel {
        /// Create the pin panel over `frame` (the image's point rect) with the
        /// given decoded capture, mirroring `PinPanel.init(image:frame:ui:contrast:)`.
        pub fn create(
            mtm: MainThreadMarker,
            image: Retained<CGImage>,
            frame: Rect,
        ) -> Retained<WSPinPanel> {
            let m = PIN_MARGIN;
            let rect = NSRect::new(
                NSPoint::new(frame.x - m, frame.y - m),
                NSSize::new(frame.w + m * 2.0, frame.h + m * 2.0),
            );
            let this = WSPinPanel::alloc(mtm)
                .set_ivars(WSPinPanelIvars { view: RefCell::new(None) });
            let panel: Retained<WSPinPanel> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: rect,
                    styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            panel.setLevel(NSFloatingWindowLevel);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(false);
            panel.setHidesOnDeactivate(false);
            panel.setMovableByWindowBackground(false);
            panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            unsafe { panel.setReleasedWhenClosed(false) };
            let view = WSPinView::create(mtm, image, rect.size);
            let base: &NSView = &view;
            panel.setContentView(Some(base));
            *panel.ivars().view.borrow_mut() = Some(view);
            panel
        }

        /// `orderFrontRegardless` + `makeKey` (mirrors `PinPanel.present`).
        pub fn present(&self) {
            self.orderFrontRegardless();
            self.makeKeyWindow();
        }

        /// `PinPanel.closePin`: order the panel out.
        pub fn close_pin(&self) {
            self.orderOut(None);
        }

        /// `PinPanel.opacity`: set the panel alpha (0.1…1).
        pub fn set_alpha(&self, a: f64) {
            self.setAlphaValue(a.clamp(0.1, 1.0));
        }

        pub fn window_number(&self) -> i64 {
            self.windowNumber() as i64
        }
    }
}

/// Owned pin windows, parallel to [`ScreenshotController::pins`]. An entry is
/// `None` when the pin was created off the main thread (model-only). `Send` is
/// asserted for the same reason as [`OverlayPanels`]: the panels are only
/// created, mutated and ordered out on the main thread.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct PinWindows(Vec<Option<objc2::rc::Retained<pin_window::WSPinPanel>>>);

#[cfg(target_os = "macos")]
unsafe impl Send for PinWindows {}

/// Owned overlay panels for the live session. `Send` is asserted because they
/// are only created, mutated and torn down on the main thread (guarded by
/// `MainThreadMarker::new()`), never from the socket accept thread.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct OverlayPanels(Vec<objc2::rc::Retained<overlay::ShotOverlayPanel>>);

#[cfg(target_os = "macos")]
unsafe impl Send for OverlayPanels {}

/// The live local keyDown monitor token. `Send` is asserted for the same reason
/// as [`OverlayPanels`]: it is installed, consulted and removed only on the main
/// thread (`MainThreadMarker::new()` guards every use). Removing the monitor
/// (`NSEvent.removeMonitor`) needs the main thread, so `Drop` only does so when
/// it is the main thread and otherwise leaves the (inert) block installed.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct KeyMonitor(Option<objc2::rc::Retained<objc2::runtime::AnyObject>>);

#[cfg(target_os = "macos")]
unsafe impl Send for KeyMonitor {}

#[cfg(target_os = "macos")]
impl Drop for KeyMonitor {
    fn drop(&mut self) {
        if let Some(m) = self.0.take() {
            if objc2::MainThreadMarker::new().is_some() {
                unsafe { objc2_app_kit::NSEvent::removeMonitor(&m) };
            }
        }
    }
}

/// Reinterpret any Objective-C object as `AnyObject` (for `downcast_ref`).
#[cfg(target_os = "macos")]
fn as_any<T: objc2::Message + ?Sized>(obj: &T) -> &objc2::runtime::AnyObject {
    unsafe { &*(obj as *const T as *const objc2::runtime::AnyObject) }
}

/// The body of `ScreenshotController.installKeyMonitor`'s block, extracted so it
/// can borrow-free the closure. Runs on the main thread.
///
/// Consumes (`null_mut`) the event when the overlay session or a pin handled it,
/// otherwise returns the raw event pointer so it keeps flowing. Never panics:
/// every ObjC-derived `Option` is guarded and no indexing can go out of bounds.
#[cfg(target_os = "macos")]
fn monitor_key_down(
    weak: &std::sync::Weak<Mutex<ScreenshotController>>,
    event: &objc2_app_kit::NSEvent,
    raw: *mut objc2_app_kit::NSEvent,
) -> *mut objc2_app_kit::NSEvent {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return raw;
    };
    // Never block the main thread on the socket accept thread's lock; if the
    // controller is busy, let the key through rather than deadlock.
    let Some(ctrl) = weak.upgrade() else {
        return raw;
    };
    let Ok(mut c) = ctrl.try_lock() else {
        return raw;
    };
    let Some(window) = event.window(mtm) else {
        return raw;
    };
    let is_overlay = as_any(&*window)
        .downcast_ref::<overlay::ShotOverlayPanel>()
        .is_some();
    let pin_numbers: Vec<i64> = c.pins.iter().map(|p| p.window_number).collect();
    match shot_key_target(is_overlay, window.windowNumber() as i64, &pin_numbers) {
        Some(ShotKeyTarget::Overlay) => {
            let mut handled = false;
            if let Some(s) = c.session.as_mut() {
                if !s.finished {
                    handled = s.handle_key(&key_input_from_event(event));
                }
            }
            // A key can finish the session (Cmd+C, Return, Esc…); drive the
            // render + delivery + teardown exactly as an in-window action would.
            if c.session.as_ref().map(|s| s.finished).unwrap_or(false) {
                c.complete_finished_session();
                return std::ptr::null_mut();
            }
            if handled {
                c.redraw_overlays();
                std::ptr::null_mut()
            } else {
                raw
            }
        }
        Some(ShotKeyTarget::Pin(i)) => {
            let action = c
                .pins
                .get_mut(i)
                .map(|pin| pin.handle_key(&pin_key_from_event(event)))
                .unwrap_or(PinKeyAction::Ignored);
            match action {
                PinKeyAction::Close => {
                    #[cfg(target_os = "macos")]
                    if let Some(Some(w)) = c.pin_windows.0.get(i) {
                        w.close_pin();
                    }
                    if i < c.pins.len() {
                        c.pins.remove(i);
                        #[cfg(target_os = "macos")]
                        if i < c.pin_windows.0.len() {
                            c.pin_windows.0.remove(i);
                        }
                    }
                }
                PinKeyAction::SetAlpha => {
                    #[cfg(target_os = "macos")]
                    if let (Some(pin), Some(Some(w))) =
                        (c.pins.get(i), c.pin_windows.0.get(i))
                    {
                        w.set_alpha(pin.alpha);
                    }
                }
                PinKeyAction::Copy | PinKeyAction::Ignored => {}
            }
            // `PinPanel.handleKey` always returns true (Swift swallows the key).
            c.drop_key_monitor_if_idle();
            std::ptr::null_mut()
        }
        None => raw,
    }
}



// ---------------------------------------------------------------------------
// ScreenshotController
// ---------------------------------------------------------------------------

pub struct ScreenshotController {
    pub config: ScreenshotConfig,
    pub state_path: String,
    pub history: ShotHistory,
    pub screens: Vec<ScreenRef>,
    pub session: Option<ShotSession>,
    pub pins: Vec<PinPanel>,
    pub last_output: Value,
    pub pane_shot_last: Value,
    pub toast: Option<ScreenToast>,
    pub log: Vec<String>,
    pub reply: Option<Vec<u8>>,
    pub prewarmed: bool,
    permission_asked: bool,
    capturing: bool,
    forced_save_path: Option<String>,
    ocr_warm: bool,
    #[cfg(target_os = "macos")]
    overlay_panels: OverlayPanels,
    /// Real pin windows, parallel to `pins` (`None` = model-only, off the main
    /// thread). Ordered out on unpin / pin close.
    #[cfg(target_os = "macos")]
    pin_windows: PinWindows,
    /// Weak self, so the key monitor block can reach the live controller
    /// without a strong cycle. Set by [`register_controller`].
    #[cfg(target_os = "macos")]
    self_ref: std::sync::Weak<Mutex<ScreenshotController>>,
    #[cfg(target_os = "macos")]
    key_monitor: KeyMonitor,
}

impl Default for ScreenshotController {
    fn default() -> Self {
        ScreenshotController::new()
    }
}

impl ScreenshotController {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        ScreenshotController {
            config: ScreenshotConfig::default(),
            state_path: format!("{home}/.cache/kitchen-sink/screenshot-state.json"),
            history: ShotHistory::default(),
            screens: Vec::new(),
            session: None,
            pins: Vec::new(),
            last_output: json!({}),
            pane_shot_last: json!({}),
            toast: None,
            log: Vec::new(),
            reply: None,
            prewarmed: false,
            permission_asked: false,
            capturing: false,
            forced_save_path: None,
            ocr_warm: false,
            #[cfg(target_os = "macos")]
            overlay_panels: OverlayPanels::default(),
            #[cfg(target_os = "macos")]
            pin_windows: PinWindows::default(),
            #[cfg(target_os = "macos")]
            self_ref: std::sync::Weak::new(),
            #[cfg(target_os = "macos")]
            key_monitor: KeyMonitor::default(),
        }
    }

    pub fn set_config(&mut self, cfg: ScreenshotConfig) {
        self.config = cfg;
    }

    /// Store the weak self-reference the key monitor block captures. Called by
    /// [`register_controller`] when the controller is wrapped in its `Arc`.
    #[cfg(target_os = "macos")]
    pub fn set_self_ref(&mut self, w: std::sync::Weak<Mutex<ScreenshotController>>) {
        self.self_ref = w;
    }

    /// `ScreenshotController.installKeyMonitor`: install the local keyDown
    /// monitor once, while a session or pin is live. No-op off the main thread
    /// or before [`set_self_ref`].
    #[cfg(target_os = "macos")]
    pub fn install_key_monitor(&mut self) {
        if self.key_monitor.0.is_some() {
            return;
        }
        if objc2::MainThreadMarker::new().is_none() {
            return;
        }
        let Some(ctrl) = self.self_ref.upgrade() else {
            return;
        };
        let weak = std::sync::Arc::downgrade(&ctrl);
        let block = block2::RcBlock::new(
            move |event: std::ptr::NonNull<objc2_app_kit::NSEvent>| -> *mut objc2_app_kit::NSEvent {
                // SAFETY: AppKit hands us a live `NSEvent` for the duration of
                // the call; we never retain it past the block.
                monitor_key_down(&weak, unsafe { event.as_ref() }, event.as_ptr())
            },
        );
        let monitor = unsafe {
            objc2_app_kit::NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                objc2_app_kit::NSEventMask::KeyDown,
                &block,
            )
        };
        self.key_monitor.0 = monitor;
    }

    /// `ScreenshotController.dropKeyMonitorIfIdle`: remove the monitor once no
    /// session and no pins remain.
    #[cfg(target_os = "macos")]
    pub fn drop_key_monitor_if_idle(&mut self) {
        if self.session.is_some() || !self.pins.is_empty() {
            return;
        }
        if let Some(m) = self.key_monitor.0.take() {
            if objc2::MainThreadMarker::new().is_some() {
                unsafe { objc2_app_kit::NSEvent::removeMonitor(&m) };
            }
        }
    }
    pub fn session(&self) -> Option<&ShotSession> {
        self.session.as_ref()
    }
    pub fn session_mut(&mut self) -> Option<&mut ShotSession> {
        self.session.as_mut()
    }
    pub fn set_session(&mut self, s: ShotSession) {
        #[cfg(target_os = "macos")]
        self.dismiss_overlay();
        self.session = Some(s);
    }
    pub fn close(&mut self) {
        #[cfg(target_os = "macos")]
        self.dismiss_overlay();
        self.session = None;
        #[cfg(target_os = "macos")]
        self.drop_key_monitor_if_idle();
    }
    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    pub fn prewarm(&mut self) {
        self.prewarmed = true;
        if self.ocr_warm {
            return;
        }
        self.ocr_warm = true;
        // `ShotOCR.warmUp`: build a small bitmap and run a recognition pass so
        // the first real OCR does not pay the model-load cost. The AppKit
        // bitmap context and Vision are main-thread only; off-main this is
        // skipped (the next `deliver_text` warm-starts lazily).
        #[cfg(target_os = "macos")]
        if objc2::MainThreadMarker::new().is_some() {
            let cfg = crate::engines::screenshot_text::ShotOCRConfig {
                languages: self.config.ocr.languages.clone(),
                correction: self.config.ocr.correction,
            };
            let _ = crate::engines::screenshot_text::ShotOCR::warm_up(&cfg);
        }
    }

    /// Build one overlay panel per display for the current session and order
    /// them in front. No-op off the main thread (the socket accept thread) or
    /// with no session.
    #[cfg(target_os = "macos")]
    fn present_overlay(&mut self) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        self.dismiss_overlay();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let ptr: *mut ShotSession = session;
        let mut panels = Vec::new();
        for (i, d) in session.displays.iter().enumerate() {
            let panel = overlay::ShotOverlayPanel::create(mtm, d.frame, i, ptr);
            panel.present();
            panels.push(panel);
        }
        if let Some(first) = panels.first() {
            first.makeKeyWindow();
        }
        self.overlay_panels.0 = panels;
        self.install_key_monitor();
    }

    /// Order out and clear every live overlay panel. Called before the session
    /// is dropped (its pointer is detached first) and on session replacement.
    #[cfg(target_os = "macos")]
    fn dismiss_overlay(&mut self) {
        for panel in self.overlay_panels.0.drain(..) {
            panel.clear_session();
            panel.orderOut(None);
        }
    }

    /// Mirror the live overlay panels' key status and window numbers into the
    /// session displays (`testState` reads the real windows). Main thread.
    #[cfg(target_os = "macos")]
    pub fn sync_live_overlay(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        for (d, panel) in s.displays.iter_mut().zip(self.overlay_panels.0.iter()) {
            d.key = panel.isKeyWindow();
            d.wid = panel.windowNumber() as i64;
        }
    }

    /// Invalidate every live overlay panel after a model change.
    #[cfg(target_os = "macos")]
    pub(crate) fn redraw_overlays(&self) {
        for panel in &self.overlay_panels.0 {
            panel.redraw();
        }
    }

    fn check_permission(&mut self, cfg: &ScreenshotConfig) -> bool {
        if screen_capture_permitted() {
            return true;
        }
        if !self.permission_asked {
            self.permission_asked = true;
            request_screen_capture_access();
        }
        ScreenToast::show(
            &mut self.toast,
            cfg.permission_toast.clone(),
            "exclamationmark.triangle.fill",
        );
        self.note("screenshot: no Screen Recording permission");
        false
    }

    pub fn trigger(&mut self, args: &ShotArgs) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            // The screen list is read fresh per capture (displays come and go).
            let screens = current_screens();
            if !screens.is_empty() {
                self.screens = screens;
            }
            if screen_capture_permitted() {
                return self.trigger_with(args, &RealCapture);
            }
        }
        self.trigger_with(args, &UnavailableCapture)
    }

    pub fn trigger_with(
        &mut self,
        args: &ShotArgs,
        capture: &dyn ScreenCapture,
    ) -> Result<(), String> {
        let cfg = self.config.clone();
        if !cfg.enabled {
            self.note("screenshot: [screenshot] enabled = false");
            return Ok(());
        }
        if self.session.is_some() || self.capturing {
            return Ok(());
        }
        if !self.check_permission(&cfg) {
            return Ok(());
        }
        self.capturing = true;
        let _ = capture.shareable_content();
        let res = capture.capture(&self.screens);
        self.capturing = false;
        match res {
            Ok(images) if !images.is_empty() => {
                match args.mode {
                    ShotMode::Gui | ShotMode::Text => {
                        let mut s =
                            ShotSession::new(cfg.clone(), args.clone(), self.state_path.clone());
                        for scr in &self.screens {
                            s.add_display(ShotDisplay::new(scr.id, scr.frame, scr.scale));
                        }
                        #[cfg(target_os = "macos")]
                        for image in &images {
                            if let Some(i) = s.displays.iter().position(|d| d.id == image.display) {
                                s.set_base_image(i, image.image.clone());
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        let _ = &images;
                        self.session = Some(s);
                        #[cfg(target_os = "macos")]
                        self.present_overlay();
                    }
                    _ => {
                        self.last_output = json!({"outcome": args.mode.as_str()});
                        self.reply = Some(Vec::new());
                    }
                }
                Ok(())
            }
            _ => {
                ScreenToast::show(
                    &mut self.toast,
                    cfg.fail_toast.clone(),
                    "exclamationmark.triangle.fill",
                );
                self.note("screenshot: capture failed");
                Ok(())
            }
        }
    }

    /// No overlay up and no capture in flight (a session has finished).
    pub fn is_idle(&self) -> bool {
        self.session.is_none() && !self.capturing
    }

    /// `ScreenshotController.handle(words:)`.
    pub fn handle(&mut self, words: &[String]) -> Result<(), String> {
        match ShotArgs::parse(words) {
            Ok(a) => self.trigger(&a),
            Err(p) => {
                self.note(format!("screenshot: {}", p.message));
                Err(p.message)
            }
        }
    }

    fn global_frame(&self) -> Option<Rect> {
        let s = self.session.as_ref()?;
        let a = s.active?;
        let r = s.selection?;
        let d = &s.displays[a];
        Some(Rect::new(
            d.frame.x + r.x,
            d.frame.max_y() - r.max_y(),
            r.w,
            r.h,
        ))
    }

    pub fn copy(&mut self, img: &ShotImage, cfg: &ScreenshotConfig, _screen: Option<ScreenRef>) {
        // PNG (+ TIFF) onto the general pasteboard, on the main thread only.
        #[cfg(target_os = "macos")]
        {
            let _ = write_image_pasteboard(img);
        }
        self.last_output["copied"] = json!(true);
        ScreenToast::show(&mut self.toast, cfg.copy_toast.clone(), "checkmark.circle.fill");
    }

    pub fn save(
        &mut self,
        img: &ShotImage,
        cfg: &ScreenshotConfig,
        _screen: Option<ScreenRef>,
        to: Option<&str>,
    ) -> Option<String> {
        let dir = expand_tilde(to.unwrap_or(&cfg.save_path));
        if to.is_none() {
            let _ = std::fs::create_dir_all(&dir);
        }
        let target = ShotFiles::target(&dir, &cfg.filename_pattern, &cfg.save_format);
        if let Some(parent) = std::path::Path::new(&target).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ext = target.rsplit('.').next().unwrap_or("").to_lowercase();
        let ok = if ["jpg", "jpeg"].contains(&ext.as_str()) {
            #[cfg(target_os = "macos")]
            {
                img.png
                    .as_deref()
                    .and_then(|png| shot_cg::reencode_png_as_jpeg(png, cfg.jpeg_quality))
                    .map(|bytes| std::fs::write(&target, &bytes).is_ok())
                    .unwrap_or(false)
            }
            #[cfg(not(target_os = "macos"))]
            {
                false
            }
        } else {
            match &img.png {
                Some(bytes) => std::fs::write(&target, bytes).is_ok(),
                None => false,
            }
        };
        if !ok {
            let t = format!("Could not save {}", abbreviate_tilde(&target));
            ScreenToast::show(&mut self.toast, t, "exclamationmark.triangle.fill");
            self.note(format!("screenshot: save failed: {target}"));
            return None;
        }
        self.last_output["path"] = json!(target);
        let toast = cfg.save_toast.replace("{}", &abbreviate_tilde(&target));
        ScreenToast::show(&mut self.toast, toast, "checkmark.circle.fill");
        self.note(format!("screenshot: saved {target}"));
        Some(target)
    }

    pub fn pin(&mut self, img: &ShotImage, frame: Option<Rect>, screen: Option<ScreenRef>) -> usize {
        let scale = screen.map(|s| s.scale).unwrap_or(2.0).max(1.0);
        let size = Size::new(img.width as f64 / scale, img.height as f64 / scale);
        let f = frame.unwrap_or(Rect::new(0.0, 0.0, size.w, size.h));
        // The model always tracks the pin; the real window is the AppKit pass.
        #[allow(unused_mut)]
        let mut panel = PinPanel::new(size, f);
        // HOST-SEAM: `do:` is dispatched on the socket accept thread, where
        // `MainThreadMarker::new()` is `None`, so no real window is created and
        // the pin stays model-only (`window_number` 0). Showing a live pin over
        // the socket needs host.rs/socket.rs to marshal `do:` onto the main
        // thread; `pin()` already does the right thing when it runs there.
        #[cfg(target_os = "macos")]
        let window = match (objc2::MainThreadMarker::new(), img.png.as_deref()) {
            (Some(mtm), Some(png)) => pin_window::decode_png(png).map(|cg| {
                let w = pin_window::WSPinPanel::create(mtm, cg, f);
                panel.window_number = w.window_number();
                panel.key = true;
                w.present();
                w
            }),
            _ => None,
        };
        self.pins.push(panel);
        #[cfg(target_os = "macos")]
        {
            self.pin_windows.0.push(window);
            self.install_key_monitor();
        }
        self.pins.len() - 1
    }

    pub fn copy_text(&mut self, text: &str, cfg: &ScreenshotConfig) {
        // NSPasteboard.string write is the AppKit pass.
        self.last_output["copied"] = json!(true);
        if !cfg.text_toast.is_empty() {
            let summary = crate::engines::screenshot_text::ShotOCR::summary(text);
            let toast = cfg.text_toast.replace("{}", &summary);
            ScreenToast::show(&mut self.toast, toast, "text.viewfinder");
        }
    }

    pub fn deliver_text(&mut self, img: &ShotImage, cfg: &ScreenshotConfig, args: &ShotArgs) {
        // Real Vision pass over the rendered capture (Swift `deliverText` →
        // `ShotOCR.text(img, cfg.ocr)`). Failures (no permission, off-main, a
        // non-decodable image) degrade to the empty string → the no-text toast.
        let started = now_millis();
        let text = ocr_text(img, &cfg.ocr);
        let ms = (now_millis() - started).max(0);
        let chars = text.chars().count();
        self.last_output = json!({
            "outcome": "text",
            "size": [img.width, img.height],
            "chars": chars,
            "text": crate::engines::screenshot_text::ShotOCR::truncate(&text, 4000),
            "ms": ms,
        });
        if text.is_empty() {
            ScreenToast::show(
                &mut self.toast,
                cfg.no_text_toast.clone(),
                "exclamationmark.triangle.fill",
            );
        } else {
            self.copy_text(&text, cfg);
        }
        self.reply = Some(if args.raw { text.into_bytes() } else { Vec::new() });
        self.note(format!(
            "screenshot: text {} chars from {}x{}",
            chars, img.width, img.height
        ));
    }

    /// Drive a finished [`ShotSession`] through the CG render and its delivery
    /// chain, mirroring `ScreenshotController.finish(_:_:)`. The pure
    /// [`ShotSession::finish`] only records the outcome; this does the
    /// render → compute screen → [`Self::end_session`] sequence and is safe to
    /// call after any interaction that may have finished (a no-op when the
    /// session is absent or still live).
    pub fn complete_finished_session(&mut self) {
        let Some(outcome) = self.session.as_ref().and_then(|s| s.outcome) else {
            return;
        };
        if !self.session.as_ref().map(|s| s.finished).unwrap_or(false) {
            return;
        }
        // Swift renders `objects: o != .text`: a text capture is rendered
        // without the annotations so OCR reads the frozen image. Abort renders
        // nothing (its image is discarded).
        let img = if outcome == ShotOutcome::Abort {
            None
        } else {
            self.session
                .as_ref()
                .and_then(|s| s.render_with(outcome != ShotOutcome::Text))
        };
        let screen = self.session.as_ref().and_then(|s| {
            s.active.map(|a| {
                let d = &s.displays[a];
                ScreenRef::new(d.id, d.frame, d.scale)
            })
        });
        self.end_session(outcome, img, screen);
    }

    /// The `ScreenshotController.finish(_:_:)` outcome routing, without the CG
    /// render (pass a [`ShotImage`]).
    pub fn end_session(
        &mut self,
        outcome: ShotOutcome,
        img: Option<ShotImage>,
        screen: Option<ScreenRef>,
    ) {
        let (cfg, args) = match &self.session {
            Some(s) => (s.cfg.clone(), s.args.clone()),
            None => (self.config.clone(), ShotArgs::default()),
        };
        let chosen_save_path = self
            .session
            .as_ref()
            .and_then(|s| s.chosen_save_path.clone());
        // Swift clears `forcedSavePath` in `finish` regardless of outcome.
        let forced_save_path = self.forced_save_path.take();
        let global_frame = self.global_frame();
        if cfg.save_last_region && outcome != ShotOutcome::Abort {
            if let Some(s) = self.session.as_ref() {
                if let (Some(a), Some(r)) = (s.active, s.selection) {
                    let d = &s.displays[a];
                    let mut st = s.state.clone();
                    st.last_region = Some(crate::engines::screenshot_annotations::ShotRegion {
                        display: d.id,
                        x: r.x,
                        y: r.y,
                        w: r.w,
                        h: r.h,
                    });
                    st.save(&s.state_path);
                }
            }
        }
        #[cfg(target_os = "macos")]
        self.dismiss_overlay();
        self.session = None;
        #[cfg(target_os = "macos")]
        self.drop_key_monitor_if_idle();
        if outcome == ShotOutcome::Abort || img.is_none() {
            self.last_output = json!({"outcome": "abort"});
            self.note("screenshot: aborted");
            self.reply = None;
            return;
        }
        let img = img.unwrap();
        if outcome == ShotOutcome::Text {
            self.deliver_text(&img, &cfg, &args);
            return;
        }
        self.last_output = json!({
            "outcome": outcome.raw_value(),
            "size": [img.width, img.height],
        });
        let action = resolve_action(outcome, &args, &cfg);
        match action {
            ShotOutcome::Copy => {
                self.copy(&img, &cfg, screen);
                if cfg.save_after_copy {
                    self.save(&img, &cfg, screen, None);
                }
            }
            ShotOutcome::Save => {
                let to = forced_save_path
                    .as_deref()
                    .or(chosen_save_path.as_deref())
                    .or(args.path.as_deref());
                self.save(&img, &cfg, screen, to);
            }
            ShotOutcome::Pin => {
                self.pin(&img, global_frame, screen);
            }
            ShotOutcome::Accept => {
                if let Some(p) = &args.path {
                    self.save(&img, &cfg, screen, Some(p));
                }
                if args.clipboard {
                    self.copy(&img, &cfg, screen);
                }
            }
            ShotOutcome::Abort | ShotOutcome::Text => {}
        }
        if let Some(png) = &img.png {
            self.history.record_png(png, cfg.history.max(0) as usize);
        }
        if args.raw {
            self.reply = img.png.clone();
        } else if args.print_geometry {
            if let Some(g) = global_frame {
                let s = format!(
                    "{} {} {} {}\n",
                    g.w.round() as i64,
                    g.h.round() as i64,
                    g.x.round() as i64,
                    g.y.round() as i64
                );
                self.reply = Some(s.into_bytes());
            }
        } else {
            self.reply = Some(Vec::new());
        }
        self.note(format!(
            "screenshot: {} {}x{}",
            action.raw_value(),
            img.width,
            img.height
        ));
    }

    // -- test hooks -------------------------------------------------------

    /// `ScreenshotController.testDo(_:)`: `Err` = the Swift error string.
    ///
    /// Runs the model hook, then — since Swift's `finish(_:)` delivers
    /// synchronously — completes a session the hook just finished (render →
    /// deliver → tear the overlay down).
    pub fn test_do(&mut self, action: &str) -> Result<(), String> {
        let result = self.test_do_model(action);
        self.complete_finished_session();
        result
    }

    fn test_do_model(&mut self, action: &str) -> Result<(), String> {
        let (verb, arg) = match action.split_once(':') {
            Some((v, a)) => (v, a),
            None => (action, ""),
        };
        match verb {
            "show" | "show-text" => {
                let mut a = ShotArgs::default();
                if verb == "show-text" {
                    a.mode = ShotMode::Text;
                }
                a.delay_ms = arg.parse().unwrap_or(0);
                self.trigger(&a)
            }
            "select" => {
                let n = nums(arg);
                if n.len() != 4 {
                    return Err("select:X,Y,W,H (overlay up)".to_string());
                }
                let Some(s) = self.session.as_mut() else {
                    return Err("select:X,Y,W,H (overlay up)".to_string());
                };
                s.test_select(Rect::new(n[0], n[1], n[2], n[3]), None);
                Ok(())
            }
            "tool" => {
                let Some(s) = self.session.as_mut() else {
                    return Err("no overlay".to_string());
                };
                if arg == "none" {
                    s.force_tool(None);
                    Ok(())
                } else {
                    match ShotTool::from_raw(arg) {
                        Some(t) if t.is_drawing() => {
                            s.force_tool(Some(t));
                            Ok(())
                        }
                        _ => Err(format!("unknown tool {arg}")),
                    }
                }
            }
            "draw" => {
                let n = nums(arg);
                if n.len() != 4 {
                    return Err("draw:X1,Y1,X2,Y2 (overlay + tool)".to_string());
                }
                let Some(s) = self.session.as_mut() else {
                    return Err("draw:X1,Y1,X2,Y2 (overlay + tool)".to_string());
                };
                s.test_draw(Point::new(n[0], n[1]), Point::new(n[2], n[3]));
                Ok(())
            }
            "key" => {
                let Some(k) = shot_key_event(arg) else {
                    return Err("key:SPEC (overlay up)".to_string());
                };
                let Some(s) = self.session.as_mut() else {
                    return Err("key:SPEC (overlay up)".to_string());
                };
                s.handle_key(&k);
                Ok(())
            }
            "copy" => {
                if let Some(s) = self.session.as_mut() {
                    s.finish(ShotOutcome::Copy);
                }
                Ok(())
            }
            "text" => {
                if let Some(s) = self.session.as_mut() {
                    s.finish(ShotOutcome::Text);
                }
                Ok(())
            }
            "mode" => {
                if arg != "text" && arg != "screenshot" {
                    return Err("mode:text|screenshot (overlay up)".to_string());
                }
                let Some(s) = self.session.as_mut() else {
                    return Err("mode:text|screenshot (overlay up)".to_string());
                };
                if s.text_mode != (arg == "text") {
                    s.toggle_text_mode();
                }
                Ok(())
            }
            "accept" => {
                if let Some(s) = self.session.as_mut() {
                    s.finish(ShotOutcome::Accept);
                }
                Ok(())
            }
            "pin" => {
                if let Some(s) = self.session.as_mut() {
                    s.finish(ShotOutcome::Pin);
                }
                Ok(())
            }
            "save" => {
                if arg.is_empty() {
                    return Err("save:PATH".to_string());
                }
                self.forced_save_path = Some(arg.to_string());
                if let Some(s) = self.session.as_mut() {
                    s.finish(ShotOutcome::Save);
                }
                Ok(())
            }
            "save-ok" => {
                let Some(field) = self
                    .session
                    .as_ref()
                    .and_then(|s| s.save_card.as_ref())
                    .map(|c| c.field.clone())
                else {
                    return Err("no save card".to_string());
                };
                if let Some(s) = self.session.as_mut() {
                    s.confirm_save(&field);
                }
                Ok(())
            }
            "close" => {
                if let Some(s) = self.session.as_mut() {
                    s.finish(ShotOutcome::Abort);
                }
                Ok(())
            }
            "unpin" => {
                #[cfg(target_os = "macos")]
                {
                    // AppKit orderOut is main-thread only; the socket `do:` path
                    // is off the main thread and holds no real windows anyway.
                    if objc2::MainThreadMarker::new().is_some() {
                        for w in self.pin_windows.0.drain(..).flatten() {
                            w.close_pin();
                        }
                    } else {
                        self.pin_windows.0.clear();
                    }
                }
                self.pins.clear();
                #[cfg(target_os = "macos")]
                self.drop_key_monitor_if_idle();
                Ok(())
            }
            "side-panel" => {
                if let Some(s) = self.session.as_mut() {
                    s.toggle_side_panel(None);
                }
                Ok(())
            }
            _ => Err(
                "show[:MS] | show-text[:MS] | mode:text|screenshot | text | select:X,Y,W,H | \
                 tool:NAME | draw:X1,Y1,X2,Y2 | key:SPEC | copy | accept | save:PATH | save-ok \
                 | pin | unpin | close | side-panel"
                    .to_string(),
            ),
        }
    }

    /// `ScreenshotController.testState` (the socket `screenshot` state).
    pub fn test_state(&self) -> Value {
        let mut st = json!({
            "shown": self.session.is_some(),
            "permission": screen_capture_permitted(),
            "pins": self.pins.len(),
            "last": self.last_output,
            "capturing": self.capturing,
            "pinWids": self.pins.iter().map(|p| p.window_number).collect::<Vec<_>>(),
            "pinStates": self
                .pins
                .iter()
                .map(|p| {
                    json!({
                        "wid": p.window_number,
                        "alpha": (p.alpha * 100.0).round() / 100.0,
                        "key": p.key,
                        "frame": [
                            p.frame.x as i64,
                            p.frame.y as i64,
                            p.frame.w as i64,
                            p.frame.h as i64,
                        ],
                    })
                })
                .collect::<Vec<_>>(),
        });
        let Some(s) = &self.session else {
            return st;
        };
        st["displays"] = json!(s
            .displays
            .iter()
            .map(|d| {
                let f = d.frame;
                json!({
                    "id": d.id as i64,
                    "frame": [f.x as i64, f.y as i64, f.w as i64, f.h as i64],
                    "scale": d.scale,
                    "key": d.key,
                    "wid": d.wid,
                    "visible": true,
                    "level": d.level,
                })
            })
            .collect::<Vec<_>>());
        if let (Some(a), Some(sel)) = (s.active, s.selection) {
            let d = &s.displays[a];
            st["selection"] = json!({
                "display": d.id as i64,
                "x": sel.x,
                "y": sel.y,
                "w": sel.w,
                "h": sel.h,
            });
            let layout =
                ButtonRing::layout(sel, d.bounds(), s.cfg.buttons.len(), s.cfg.button_size);
            st["buttons"] = json!(s
                .cfg
                .buttons
                .iter()
                .zip(layout.frames.iter())
                .map(|(t, f)| json!({
                    "name": t.raw_value(),
                    "x": f.x,
                    "y": f.y,
                    "w": f.w,
                    "h": f.h,
                }))
                .collect::<Vec<_>>());
        } else {
            st["selection"] = Value::Null;
            st["buttons"] = json!([]);
        }
        st["tool"] = match s.tool {
            Some(t) => json!(t.raw_value()),
            None => Value::Null,
        };
        st["moveMode"] = json!(s.move_mode);
        st["textMode"] = json!(s.text_mode);
        st["modePill"] = Value::Null;
        st["size"] = match s.active_size() {
            Some(v) => json!(v),
            None => Value::Null,
        };
        st["color"] = json!(s.current_color().hex());
        st["objects"] = json!(s
            .doc
            .objects
            .iter()
            .map(|o| {
                let b = o.bbox();
                let mut x = json!({
                    "type": o.tool.raw_value(),
                    "bbox": [b.x, b.y, b.w, b.h],
                    "color": o.color.hex(),
                    "size": o.size,
                });
                if o.tool == ShotTool::Counter {
                    x["number"] = json!(o.number);
                }
                x
            })
            .collect::<Vec<_>>());
        st["selected"] = match s.doc.selected {
            Some(i) => json!(i),
            None => Value::Null,
        };
        st["canUndo"] = json!(s.doc.can_undo());
        st["canRedo"] = json!(s.doc.can_redo());
        st["sidePanel"] = json!(s.side_panel_open);
        st["helpShown"] = json!(s.cfg.show_help && s.selection.is_none());
        st["editingText"] = json!(s.editing.is_some());
        st["wheel"] = json!(s.wheel.is_some());
        st["grabbing"] = json!(s.grabbing.is_some());
        st["saveCard"] = match &s.save_card {
            Some(c) => json!({"path": c.field, "key": true}),
            None => Value::Null,
        };
        st
    }
}

/// `ShotOCR.text(img, cfg.ocr)` over a rendered capture: decode the PNG back to
/// a `CGImage`, run `VNRecognizeTextRequest`, and join the lines.
///
/// Never errors. AppKit/Vision run on the main thread only (the macOS image
/// seam this module shares with `pin_window::decode_png`); off the main
/// thread, with no Screen Recording permission, or with bytes that are not a
/// decodable image, this degrades to the empty string — the "no text" path.
fn ocr_text(img: &ShotImage, ocr: &ShotOCRConfigModel) -> String {
    #[cfg(target_os = "macos")]
    {
        if objc2::MainThreadMarker::new().is_none() {
            return String::new();
        }
        let Some(png) = img.png.as_deref() else {
            return String::new();
        };
        let Some(image) = pin_window::decode_png(png) else {
            return String::new();
        };
        let cfg = crate::engines::screenshot_text::ShotOCRConfig {
            languages: ocr.languages.clone(),
            correction: ocr.correction,
        };
        let lines = crate::engines::screenshot_text::ShotOCR::recognize(&image, &cfg);
        crate::engines::screenshot_text::ShotOCR::text(&lines)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (img, ocr);
        String::new()
    }
}

/// Write the capture's PNG (and a decoded TIFF, when derivable) to the general
/// pasteboard. Main-thread only; returns `false` off the main thread, with no
/// image bytes, or when the pasteboard rejects the write.
#[cfg(target_os = "macos")]
fn write_image_pasteboard(img: &ShotImage) -> bool {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
        NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSData};

    if objc2::MainThreadMarker::new().is_none() {
        return false;
    }
    let Some(png) = img.png.as_deref() else {
        return false;
    };
    let item = NSPasteboardItem::new();
    item.setData_forType(&NSData::with_bytes(png), unsafe { NSPasteboardTypePNG });
    if let Some(tiff) = png_to_tiff(png) {
        item.setData_forType(&NSData::with_bytes(&tiff), unsafe { NSPasteboardTypeTIFF });
    }
    let writable: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
        ProtocolObject::from_retained(item);
    let objects = NSArray::from_retained_slice(&[writable]);
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.writeObjects(&objects)
}

/// Decode a PNG and re-encode it as TIFF (the `rep.tiffRepresentation` half of
/// `ScreenshotController.copy`). Works off the main thread.
#[cfg(target_os = "macos")]
fn png_to_tiff(png: &[u8]) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::NSBitmapImageRep;
    use objc2_foundation::NSData;

    let data = NSData::with_bytes(png);
    let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)?;
    rep.TIFFRepresentation().map(|t| t.to_vec())
}

fn nums(s: &str) -> Vec<f64> {
    s.split(',')
        .filter_map(|p| p.trim().parse::<f64>().ok())
        .collect()
}

/// Run one `do:screenshot:*` body against the shared controller and return the
/// socket reply (`null` ok, `{"error": …}` otherwise). Split out so the host can
/// run it on the main thread (Swift's `testQuery` main hop) — the registry hook
/// below uses it directly for headless/test callers.
pub fn controller_test_do(
    controller: &Arc<Mutex<ScreenshotController>>,
    rest: &str,
) -> Value {
    let Ok(mut c) = controller.lock() else {
        return json!({"error": "controller busy"});
    };
    let result = c.test_do(rest);
    #[cfg(target_os = "macos")]
    c.redraw_overlays();
    match result {
        Ok(()) => Value::Null,
        Err(e) => json!({"error": e}),
    }
}

/// Register the `do:screenshot:*` hooks into the shared [`Registry`].
pub fn register_controller(reg: &mut Registry, controller: Arc<Mutex<ScreenshotController>>) {
    #[cfg(target_os = "macos")]
    if let Ok(mut c) = controller.lock() {
        c.set_self_ref(Arc::downgrade(&controller));
    }
    reg.register_test_do(move |action: &str| {
        let rest = action.strip_prefix("screenshot:")?;
        Some(controller_test_do(&controller, rest))
    });
}


// ---------------------------------------------------------------------------
// pane-shot (`ScreenshotController.paneShot`)
// ---------------------------------------------------------------------------

/// One rendered pane-shot (`PaneShotImage`).
pub struct PaneShotImage {
    pub image: ShotImage,
    pub rows: usize,
    pub title: String,
    pub pane: Option<String>,
}

/// `ScreenshotController.paneShotImage`: read the pane through herdr (or
/// `--file`), parse the ANSI, render it with the Ghostty theme. Safe off the
/// main thread (CoreGraphics only).
#[cfg(target_os = "macos")]
pub fn pane_shot_image(
    args: &crate::engines::pane_shot::PaneShotArgs,
    cfg: &crate::engines::pane_shot::PaneShotConfig,
    scale: f64,
) -> Result<PaneShotImage, crate::engines::pane_shot::Failure> {
    use crate::engines::ansi_render::{AnsiGrid, AnsiRGB, AnsiRender, AnsiTheme};
    use crate::engines::pane_shot::{self as ps, Failure, Herdr};
    let (text, title, pane) = if let Some(file) = &args.file {
        let p = ps::expand_tilde(file);
        let t = std::fs::read_to_string(&p).map_err(|_| Failure::new(format!("can't read {file}")))?;
        let name = std::path::Path::new(&p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.clone());
        (t, name, None)
    } else {
        let p = Herdr::pane(&cfg.herdr_bin, args.pane.as_deref())?;
        let n = Herdr::lines(p.viewport_rows, args.lines.unwrap_or(cfg.lines), args.all);
        let t = Herdr::read(&cfg.herdr_bin, &p.id, n)?;
        (t, p.title, Some(p.id))
    };
    let grid = AnsiGrid::parse(&text);
    if grid.rows.is_empty() {
        return Err(Failure::new(format!("nothing to capture: {title} is empty")));
    }
    let mut theme = AnsiTheme::default();
    let ghostty = ps::expand_tilde(&cfg.ghostty_bin);
    if ps::is_executable_file(&ghostty) {
        if let Ok(r) = crate::app::process_run::run_process(
            &ghostty,
            &["+show-config".to_string()],
            None,
            None,
            false,
        ) {
            if r.code == 0 {
                theme = AnsiTheme::ghostty(&r.out);
            }
        }
    }
    if !cfg.font.is_empty() {
        theme.font_name = cfg.font.clone();
    }
    if cfg.font_size > 0.0 {
        theme.font_size = cfg.font_size;
    }
    if let Some(bg) = AnsiRGB::from_hex(&cfg.background) {
        theme.background = bg;
    }
    let rows = grid.rows.len();
    let failed = || Failure::new(format!("could not render {rows} rows"));
    let img = AnsiRender::image(&grid, &theme, cfg.padding, scale).ok_or_else(failed)?;
    let png = shot_cg::encode_png(img.as_ref()).ok_or_else(failed)?;
    Ok(PaneShotImage {
        image: ShotImage::with_png(img.width() as i64, img.height() as i64, png),
        rows,
        title,
        pane,
    })
}

impl ScreenshotController {
    /// `deliverPaneShot`: save and/or copy per `--save`/`--copy` over the
    /// `[pane-shot]` defaults; returns the socket reply line (the saved path,
    /// `copied`, or `error: …`). The floating preview is not ported.
    pub fn deliver_pane_shot(
        &mut self,
        shot: &PaneShotImage,
        args: &crate::engines::pane_shot::PaneShotArgs,
        cfg: &crate::engines::pane_shot::PaneShotConfig,
        started_ms: i64,
    ) -> String {
        let copy = args.copy.unwrap_or(cfg.copy);
        let save = args.save.unwrap_or(cfg.save);
        let toast = cfg
            .toast
            .replace("{n}", &shot.rows.to_string())
            .replace("{pane}", &shot.title);
        let mut sc = ScreenshotConfig::load();
        sc.copy_path_after_save = false;
        sc.filename_pattern = cfg.filename_pattern.clone();
        sc.save_format = "png".to_string();
        if !cfg.save_path.is_empty() {
            sc.save_path = cfg.save_path.clone();
        }
        let path = if save {
            sc.save_toast = if copy { String::new() } else { "Saved {}".to_string() };
            self.save(&shot.image, &sc, None, None)
        } else {
            None
        };
        if copy {
            sc.copy_toast = toast;
            self.copy(&shot.image, &sc, None);
        }
        if cfg.preview {
            self.note("pane-shot: preview window not ported");
        }
        let ms = now_millis() - started_ms;
        self.pane_shot_last = json!({
            "pane": shot.pane.clone().unwrap_or_default(),
            "title": shot.title,
            "rows": shot.rows,
            "size": [shot.image.width, shot.image.height],
            "copied": copy,
            "path": path.clone().unwrap_or_default(),
            "ms": ms,
        });
        self.note(format!(
            "pane-shot {}: {} rows, {}×{} px, {} ms",
            shot.pane.as_deref().unwrap_or("file"),
            shot.rows,
            shot.image.width,
            shot.image.height,
            ms
        ));
        if save && path.is_none() {
            return "error: could not save".to_string();
        }
        path.unwrap_or_else(|| "copied".to_string())
    }

    /// A pane-shot that failed before delivery (`paneShotLast = ["error": …]`).
    pub fn pane_shot_failed(&mut self, message: &str) {
        self.pane_shot_last = json!({ "error": message });
        self.note(format!("pane-shot: {message}"));
        ScreenToast::show(&mut self.toast, message.to_string(), "exclamationmark.triangle.fill");
    }
}

/// The backing scale of the screen under the mouse (`mouseScreen`), else the
/// main screen's; 2 off the main thread.
#[cfg(target_os = "macos")]
pub fn mouse_screen_scale() -> f64 {
    use objc2_app_kit::{NSEvent, NSScreen};
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return 2.0;
    };
    let p = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let hit = screens.iter().find(|s| {
        let f = s.frame();
        p.x >= f.origin.x && p.x < f.origin.x + f.size.width && p.y >= f.origin.y && p.y < f.origin.y + f.size.height
    });
    hit.or_else(|| NSScreen::mainScreen(mtm))
        .map(|s| s.backingScaleFactor())
        .unwrap_or(2.0)
}

pub(crate) fn epoch_millis() -> i64 {
    now_millis()
}

// __PART_G__

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::registry::Registry;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn entries(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn temp_path(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("ws-shot-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        format!("{}/state.json", dir.to_string_lossy())
    }

    fn session() -> ShotSession {
        let mut s = ShotSession::new(
            ScreenshotConfig::default(),
            ShotArgs::default(),
            temp_path("sess"),
        );
        s.add_display(ShotDisplay::new(
            1,
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            2.0,
        ));
        s
    }

    #[test]
    fn config_defaults_and_parsing() {
        let d = ScreenshotConfig::default();
        assert!(d.enabled);
        assert_eq!(d.button_size, 34.0);
        assert_eq!(d.buttons.len(), 21, "default ring = 20 + size badge");
        assert_eq!(d.save_path, "~/Desktop");
        assert_eq!(d.user_colors.first(), Some(&None), "picker swatch first");

        let c = ScreenshotConfig::from_entries(&entries(&[
            ("enabled", "false"),
            ("ui-color", "#112233"),
            ("return", "pin"),
            ("save-format", "jpeg"),
            ("history", "5000"),
            ("button-size", "30"),
            ("start-mode", "text"),
            ("text-languages", "en, ja"),
            ("show-help", "no"),
        ]));
        assert!(!c.enabled);
        assert_eq!(c.ui_color.hex(), "#112233");
        assert_eq!(c.return_action, "pin");
        assert_eq!(c.save_format, "jpg");
        assert_eq!(c.history, 500, "history clamped 0…500");
        assert_eq!(c.button_size, 30.0);
        assert!(c.start_text);
        assert_eq!(c.ocr.languages, vec!["en".to_string(), "ja".to_string()]);
        assert!(!c.show_help);

        let bad = ScreenshotConfig::from_entries(&entries(&[("return", "nope"), ("save-format", "webp")]));
        assert_eq!(bad.return_action, "copy", "invalid return falls back");
        assert_eq!(bad.save_format, "png", "invalid format falls back");
        assert_eq!(bad.button_size, 34.0, "0 = automatic size");

        let theme = ScreenshotConfig::from_entries(&entries(&[("ui-color", "theme")]));
        assert_eq!(theme.ui_color, d.ui_color, "'theme' keeps the default");
    }

    #[test]
    fn history_records_and_prunes() {
        let dir = std::env::temp_dir().join(format!("shot-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_string_lossy().to_string();
        for n in [
            "shot-20260101-000000-000.png",
            "shot-20260102-000000-000.png",
            "shot-20260103-000000-000.png",
        ] {
            ShotHistory::record_png_at(&d, b"x", n, 99).unwrap();
        }
        // A non-shot file must be ignored.
        std::fs::write(format!("{d}/other.png"), b"y").unwrap();

        let list = ShotHistory::entries_in(&d);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "shot-20260103-000000-000.png", "newest first");

        let removed = ShotHistory::prune_in(&d, 2);
        assert_eq!(removed.len(), 1, "oldest pruned");
        assert!(removed[0].ends_with("shot-20260101-000000-000.png"));
        assert_eq!(ShotHistory::entries_in(&d).len(), 2);
        assert!(std::path::Path::new(&format!("{d}/other.png")).exists());

        assert!(ShotHistory::record_png_at(&d, b"x", "shot-new.png", 0).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shot_key_target_resolves_overlay_and_pins() {
        // An overlay window always wins (mirrors `e.window is ShotOverlayPanel`).
        assert_eq!(
            shot_key_target(true, 42, &[7, 42]),
            Some(ShotKeyTarget::Overlay)
        );
        // Otherwise a matching pin `windowNumber` resolves to that pin.
        assert_eq!(shot_key_target(false, 7, &[7, 42]), Some(ShotKeyTarget::Pin(0)));
        assert_eq!(
            shot_key_target(false, 42, &[7, 42]),
            Some(ShotKeyTarget::Pin(1))
        );
        // No mask (number 0) never matches, and neither does an unknown window.
        assert_eq!(shot_key_target(false, 0, &[0, 0]), None);
        assert_eq!(shot_key_target(false, 99, &[7, 42]), None);
        assert_eq!(shot_key_target(false, 5, &[]), None);
    }

    #[test]
    fn pin_panel_model() {
        let mut p = PinPanel::new(Size::new(200.0, 100.0), Rect::new(0.0, 0.0, 212.0, 112.0));
        assert_eq!(p.alpha, 1.0);
        p.handle_key(&PinKey { key_code: 0, ch: Some('5'), cmd: false });
        assert!((p.alpha - 0.5).abs() < 1e-9, "5 → 50% opacity");
        p.handle_key(&PinKey { key_code: 0, ch: Some('0'), cmd: false });
        assert_eq!(p.alpha, 1.0, "0 → full opacity");
        p.handle_key(&PinKey { key_code: 0, ch: Some('9'), cmd: false });
        assert!((p.alpha - 0.1).abs() < 1e-9, "9 → 10% opacity");

        p.set_zoom(8.0, None);
        assert_eq!(p.zoom, 8.0, "zoom clamped at 8");
        p.rotate(1);
        assert_eq!((p.base.w, p.base.h), (100.0, 200.0), "quarter turn swaps base");
        p.rotate(2);
        assert_eq!((p.base.w, p.base.h), (100.0, 200.0), "even turns keep the base");

        assert_eq!(
            p.handle_key(&PinKey { key_code: 53, ch: None, cmd: false }),
            PinKeyAction::Close
        );
        assert_eq!(
            p.handle_key(&PinKey { key_code: 8, ch: Some('c'), cmd: true }),
            PinKeyAction::Copy
        );
        assert_eq!(
            p.handle_key(&PinKey { key_code: 8, ch: Some('c'), cmd: false }),
            PinKeyAction::Ignored
        );
    }

    #[test]
    fn session_esc_chain_peels_one_layer_at_a_time() {
        let mut s = session();
        s.test_select(Rect::new(100.0, 100.0, 200.0, 200.0), Some(0));
        assert_eq!(s.selection.map(|r| r.w), Some(200.0));

        s.doc.add(ShotObject::new(
            ShotTool::Line,
            vec![Point::new(0.0, 0.0), Point::new(10.0, 10.0)],
            ShotColor::BLACK,
            3,
        ));
        s.set_tool(Some(ShotTool::Pencil));
        s.doc.selected = Some(0);
        s.toggle_side_panel(Some(true));
        s.start_grab();
        s.wheel = Some(WheelState {
            display: 0,
            center: Point::ZERO,
            hot: None,
        });
        s.show_shortcuts();
        s.begin_text(0, Point::new(5.0, 5.0), None);
        s.save_card = Some(SaveCardState {
            display: 0,
            field: "/tmp/x.png".to_string(),
        });

        s.escape();
        assert!(s.save_card.is_none(), "1: save card closed");
        s.escape();
        assert!(s.editing.is_none(), "2: text committed");
        s.escape();
        assert!(!s.shortcuts_shown, "3: shortcuts hidden");
        s.escape();
        assert!(s.wheel.is_none(), "4: wheel closed");
        s.escape();
        assert!(s.grabbing.is_none(), "5: grab cancelled");
        s.escape();
        assert!(!s.side_panel_open, "6: side panel closed");
        s.escape();
        assert!(s.doc.selected.is_none(), "7: selection cleared");
        s.escape();
        assert!(s.tool.is_none(), "8: tool cleared");
        assert!(!s.finished);
        s.escape();
        assert!(s.finished, "9: overlay finishes (abort)");
        assert_eq!(s.outcome, Some(ShotOutcome::Abort));
    }

    #[test]
    fn session_key_routing_tools_text_mode() {
        let mut s = session();
        s.test_select(Rect::new(10.0, 10.0, 100.0, 100.0), Some(0));

        // Tool letter.
        assert!(s.handle_key(&shot_key_event("p").unwrap()));
        assert_eq!(s.tool, Some(ShotTool::Pencil));
        // Esc clears the tool (does not close yet).
        assert!(s.handle_key(&shot_key_event("esc").unwrap()));
        assert_eq!(s.tool, None);
        assert!(!s.finished);

        // Tab toggles text mode (key code 48).
        let tab = KeyInput {
            key_code: 48,
            chars: None,
            cmd: false,
            ctrl: false,
            shift: false,
            opt: false,
            esc_streak: 0,
        };
        assert!(s.handle_key(&tab));
        assert!(s.text_mode);
        assert!(s.handle_key(&shot_key_event("o").unwrap()));
        assert!(!s.text_mode, "'o' toggles back");

        // Cmd+C finishes as copy only when a selection exists.
        let cmd_c = shot_key_event("cmd+c").unwrap();
        assert!(s.handle_key(&cmd_c));
        assert!(s.finished);
        assert_eq!(s.outcome, Some(ShotOutcome::Copy));
    }

    #[test]
    fn resolve_action_accept_branches() {
        let cfg = ScreenshotConfig::default();
        let mut args = ShotArgs::default();
        assert_eq!(
            resolve_action(ShotOutcome::Accept, &args, &cfg),
            ShotOutcome::Copy,
            "accept + no flags = the return action (copy)"
        );
        args.pin = true;
        assert_eq!(resolve_action(ShotOutcome::Accept, &args, &cfg), ShotOutcome::Pin);
        args.pin = false;
        args.clipboard = true;
        assert_eq!(
            resolve_action(ShotOutcome::Accept, &args, &cfg),
            ShotOutcome::Accept
        );
        assert_eq!(
            resolve_action(ShotOutcome::Save, &args, &cfg),
            ShotOutcome::Save,
            "non-accept passes through"
        );
        assert_eq!(ShotOutcome::from_raw("text"), Some(ShotOutcome::Text));
        assert_eq!(ShotOutcome::from_raw("nope"), None);
        assert_eq!(ShotOutcome::Text.raw_value(), "text");
    }

    #[test]
    fn controller_state_json_shape() {
        let mut c = ScreenshotController::new();
        let idle = c.test_state();
        assert_eq!(idle["shown"], false);
        assert_eq!(idle["pins"], 0);
        assert_eq!(idle["capturing"], false);
        assert!(idle["permission"].is_boolean());
        assert_eq!(idle["last"], json!({}));
        assert_eq!(idle["pinStates"], json!([]));
        assert!(idle.get("displays").is_none(), "no session = no displays");

        let mut s = session();
        s.test_select(Rect::new(10.0, 20.0, 300.0, 200.0), Some(0));
        c.set_session(s);
        let st = c.test_state();
        assert_eq!(st["shown"], true);
        assert_eq!(st["displays"][0]["id"], 1);
        assert_eq!(st["displays"][0]["level"], 1000);
        assert_eq!(st["selection"]["display"], 1);
        assert_eq!(st["selection"]["w"], 300.0);
        assert_eq!(
            st["buttons"].as_array().unwrap().len(),
            21,
            "one button per ring tool"
        );
        assert_eq!(st["buttons"][0]["name"], "pencil");
        assert_eq!(st["tool"], Value::Null);
        assert_eq!(st["canUndo"], false);
        assert_eq!(st["canRedo"], false);
        assert_eq!(st["sidePanel"], false);
        assert_eq!(st["helpShown"], false, "a selection hides the help");
        assert_eq!(st["editingText"], false);
        assert_eq!(st["objects"], json!([]));
        assert_eq!(st["selected"], Value::Null);
        assert_eq!(st["saveCard"], Value::Null);
        assert_eq!(st["color"], "#ff0000");
        assert_eq!(st["size"], Value::Null);
    }

    #[test]
    fn controller_test_do_dispatch() {
        let mut c = ScreenshotController::new();
        assert!(c.test_do("select:1,2,3,4").is_err(), "no overlay");
        assert!(c.test_do("tool:pencil").is_err(), "no overlay");
        assert!(c.test_do("draw:1,2,3,4").is_err(), "no overlay");
        assert!(c.test_do("key:esc").is_err(), "no overlay");
        assert!(c.test_do("bogus").is_err(), "unknown verb");
        assert_eq!(c.test_do("save:").unwrap_err(), "save:PATH");
        assert!(c.test_do("close").is_ok(), "close with no session is a no-op");

        c.set_session(session());
        assert!(c.test_do("select:10,20,100,50").is_ok());
        assert_eq!(c.session().unwrap().selection.unwrap().w, 100.0);
        assert!(c.test_do("tool:pencil").is_ok());
        assert_eq!(c.session().unwrap().tool, Some(ShotTool::Pencil));
        assert!(c.test_do("tool:none").is_ok());
        assert_eq!(c.session().unwrap().tool, None);
        assert!(c.test_do("tool:nonsense").is_err(), "not a drawing tool");
        assert!(c.test_do("mode:text").is_ok());
        assert!(c.session().unwrap().text_mode);
        assert!(c.test_do("mode:bogus").is_err());
        assert!(c.test_do("key:esc").is_ok());

        // A disabled config short-circuits `show` (no capture backend needed).
        c.session = None;
        c.set_config(ScreenshotConfig {
            enabled: false,
            ..ScreenshotConfig::default()
        });
        assert!(c.test_do("show").is_ok());
        assert!(c.log.iter().any(|l| l.contains("enabled = false")));
    }

    #[test]
    fn controller_registry_dispatch() {
        let ctrl = Arc::new(Mutex::new(ScreenshotController::new()));
        ctrl.lock().unwrap().set_session(session());
        let mut reg = Registry::new();
        register_controller(&mut reg, ctrl);

        let ok = reg.dispatch_test_do("screenshot:select:1,2,3,4").unwrap();
        assert_eq!(ok, Value::Null, "ok hook = null");
        let err = reg.dispatch_test_do("screenshot:bogus").unwrap();
        assert!(err["error"].as_str().unwrap().contains("show[:MS]"));
        assert!(reg.dispatch_test_do("not-screenshot").is_none());
    }

    #[test]
    fn controller_delivery_copy_save_pin() {
        let mut c = ScreenshotController::new();
        let cfg = ScreenshotConfig::default();
        let img = ShotImage::with_png(40, 30, b"PNGDATA".to_vec());

        c.copy(&img, &cfg, None);
        assert_eq!(c.last_output["copied"], true);
        assert_eq!(c.toast.as_ref().unwrap().text, "Capture saved to clipboard");

        let dir = std::env::temp_dir().join(format!("shot-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = c.save(&img, &cfg, None, Some(dir.to_str().unwrap()));
        let target = target.expect("saved");
        assert!(std::path::Path::new(&target).exists());
        assert!(target.ends_with(".png"));
        assert_eq!(std::fs::read(&target).unwrap(), b"PNGDATA");
        let _ = std::fs::remove_dir_all(&dir);

        let idx = c.pin(&img, None, None);
        assert_eq!(idx, 0);
        assert_eq!(c.pins.len(), 1);
        let st = c.test_state();
        assert_eq!(st["pins"], 1);
        assert_eq!(st["pinStates"][0]["alpha"], 1.0);
    }

    #[test]
    fn end_session_abort_and_copy() {
        let mut c = ScreenshotController::new();
        c.set_session(session());
        c.end_session(ShotOutcome::Abort, None, None);
        assert_eq!(c.last_output, json!({"outcome": "abort"}));
        assert!(c.session.is_none());

        let mut c = ScreenshotController::new();
        let mut s = session();
        s.test_select(Rect::new(0.0, 0.0, 100.0, 100.0), Some(0));
        c.set_session(s);
        c.end_session(
            ShotOutcome::Copy,
            Some(ShotImage::with_png(100, 100, b"x".to_vec())),
            None,
        );
        assert_eq!(c.last_output["outcome"], "copy");
        assert_eq!(c.last_output["copied"], true);
        assert!(c.session.is_none());

        // Print-geometry reply is "w h x y".
        let mut c = ScreenshotController::new();
        let mut s = session();
        s.test_select(Rect::new(10.0, 20.0, 30.0, 40.0), Some(0));
        let mut args = ShotArgs::default();
        args.print_geometry = true;
        s.args = args;
        c.set_session(s);
        c.end_session(ShotOutcome::Accept, Some(ShotImage::new(30, 40)), None);
        assert_eq!(
            String::from_utf8(c.reply.clone().unwrap()).unwrap(),
            "30 40 10 840\n"
        );
    }

    #[test]
    fn screen_ref_bounds_is_origin_rect() {
        let r = ScreenRef::new(1, Rect::new(100.0, 50.0, 1440.0, 900.0), 2.0);
        assert_eq!(r.id, 1);
        let b = r.bounds();
        assert_eq!((b.x, b.y, b.w, b.h), (0.0, 0.0, 1440.0, 900.0));
    }

    #[test]
    fn captured_image_carries_scale_and_geometry() {
        let img = CapturedImage {
            display: 3,
            width: 2880,
            height: 1800,
            scale: 2.0,
            #[cfg(target_os = "macos")]
            image: None,
        };
        let dbg = format!("{img:?}");
        assert!(dbg.contains("CapturedImage"), "{dbg}");
        assert!(dbg.contains("2880"), "{dbg}");
        assert_eq!(img.scale, 2.0);
        assert_eq!(img.width * img.height, 5_184_000);
    }

    #[test]
    fn unavailable_capture_reports_clear_errors() {
        let c = UnavailableCapture;
        assert!(c.shareable_content().is_err());
        let err = c.capture(&[]).unwrap_err();
        assert_eq!(
            err.0,
            "ScreenCaptureKit unavailable: SCScreenshotManager not captured"
        );
    }

    #[test]
    fn render_geometry_and_scale_math() {
        assert_eq!(canvas_scale(2880, 1440.0), 2.0, "retina \u{d7}2");
        assert_eq!(canvas_scale(1440, 1440.0), 1.0, "\u{d7}1");
        assert_eq!(canvas_scale(100, 1440.0), 1.0, "clamped at \u{2265} 1");
        assert_eq!(canvas_scale(2880, 0.0), 2880.0, "no frame width = raw width");

        let r = integral(Rect::new(1.2, 2.8, 3.4, 0.5));
        assert_eq!((r.x, r.y, r.w, r.h), (1.0, 2.0, 4.0, 2.0), "CGRect.integral");

        let px = px_rect(Rect::new(10.0, 20.0, 30.0, 40.0), 2.0);
        assert_eq!((px.x, px.y, px.w, px.h), (20.0, 40.0, 60.0, 80.0), "points \u{d7} scale");

        assert_eq!(output_size(Rect::new(0.0, 0.0, 30.0, 40.0), 2.0), (60, 80));
        assert_eq!(output_size(Rect::new(0.0, 0.0, 3.0, 3.0), 1.5), (5, 5), "round half away");
    }

    #[test]
    fn render_without_a_base_image_is_none() {
        let mut s = session();
        s.test_select(Rect::new(0.0, 0.0, 100.0, 100.0), Some(0));
        assert!(s.render().is_none(), "no frozen capture = nothing to draw");
    }

    #[test]
    fn selection_handles_geometry() {
        let s = Rect::new(10.0, 20.0, 100.0, 50.0);
        let h = selection_handles(s);
        assert_eq!(h.len(), 8);
        assert_eq!((h[0].x, h[0].y), (10.0, 20.0), "top-left");
        assert_eq!((h[1].x, h[1].y), (60.0, 20.0), "top-mid");
        assert_eq!((h[2].x, h[2].y), (110.0, 20.0), "top-right");
        assert_eq!((h[3].x, h[3].y), (110.0, 45.0), "right-mid");
        assert_eq!((h[4].x, h[4].y), (110.0, 70.0), "bottom-right");
        assert_eq!((h[5].x, h[5].y), (60.0, 70.0), "bottom-mid");
        assert_eq!((h[6].x, h[6].y), (10.0, 70.0), "bottom-left");
        assert_eq!((h[7].x, h[7].y), (10.0, 45.0), "left-mid");
    }

    #[test]
    fn selection_handle_hit_test() {
        let s = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(selection_handle_at(s, Point::new(0.0, 0.0), 34.0), Some(0));
        assert_eq!(
            selection_handle_at(s, Point::new(100.0, 100.0), 34.0),
            Some(4)
        );
        assert_eq!(selection_handle_at(s, Point::new(50.0, 0.0), 34.0), Some(1));
        assert_eq!(
            selection_handle_at(s, Point::new(50.0, 50.0), 34.0),
            None,
            "the centre is not a handle"
        );
        // Radius = max(6, size*0.3) + 2 = 12.2 at the default button size.
        assert_eq!(
            selection_handle_at(s, Point::new(0.0, 7.0), 34.0),
            Some(0),
            "inside the hit radius"
        );
        assert_eq!(
            selection_handle_at(s, Point::new(0.0, 20.0), 34.0),
            None,
            "outside the hit radius"
        );
    }

    #[test]
    fn save_jpeg_without_png_bytes_fails_cleanly() {
        let mut c = ScreenshotController::new();
        let mut cfg = ScreenshotConfig::default();
        cfg.save_format = "jpg".to_string();
        let dir = std::env::temp_dir().join(format!("shot-jpeg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = ShotImage::new(4, 4); // no encoded bytes
        let out = c.save(&img, &cfg, None, Some(dir.to_str().unwrap()));
        assert!(out.is_none(), "nothing to re-encode = save fails, no panic");
        assert!(c
            .toast
            .as_ref()
            .map(|t| t.text.starts_with("Could not save"))
            .unwrap_or(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pin_numbering_and_live_window_ids() {
        let mut c = ScreenshotController::new();
        // Synthetic (non-decodable) bytes: off the main thread this stays a
        // model-only pin (`window_number` 0), like the socket `do:` path.
        let img = ShotImage::with_png(40, 30, b"not-an-image".to_vec());
        assert_eq!(c.pin(&img, None, None), 0, "first pin index");
        assert_eq!(c.pin(&img, None, None), 1, "second pin index");
        assert_eq!(c.pins.len(), 2);

        let st = c.test_state();
        assert_eq!(st["pins"], 2);
        assert_eq!(st["pinWids"], json!([0, 0]), "off-main = no window number");
        assert_eq!(st["pinStates"][1]["wid"], 0);

        // A real window number (set on the main thread) surfaces verbatim.
        c.pins[1].window_number = 4242;
        c.pins[1].key = true;
        let st = c.test_state();
        assert_eq!(st["pinWids"], json!([0, 4242]));
        assert_eq!(st["pinStates"][1]["wid"], 4242);
        assert_eq!(st["pinStates"][1]["key"], true);

        // `unpin` clears the models (and, on macOS, the parallel windows).
        assert!(c.test_do("unpin").is_ok());
        assert!(c.pins.is_empty());
        assert_eq!(c.test_state()["pinWids"], json!([]));
    }

    #[test]
    fn complete_finished_session_drives_outcomes() {
        // A live (unfinished) session is left alone.
        let mut c = ScreenshotController::new();
        c.set_session(session());
        c.complete_finished_session();
        assert!(c.session.is_some(), "not finished = no teardown");

        // Abort renders nothing, tears the session down and records abort.
        let mut c = ScreenshotController::new();
        let mut s = session();
        s.test_select(Rect::new(0.0, 0.0, 100.0, 50.0), Some(0));
        c.set_session(s);
        c.session_mut().unwrap().finish(ShotOutcome::Abort);
        c.complete_finished_session();
        assert!(c.session.is_none(), "abort tears the overlay down");
        assert_eq!(c.last_output, json!({"outcome": "abort"}));

        // A non-abort finish with no frozen capture degrades to abort (no
        // render) but still tears the session down — never panics.
        let mut c = ScreenshotController::new();
        let mut s = session();
        s.test_select(Rect::new(0.0, 0.0, 100.0, 50.0), Some(0));
        c.set_session(s);
        c.session_mut().unwrap().finish(ShotOutcome::Copy);
        c.complete_finished_session();
        assert!(c.session.is_none());
        assert_eq!(c.last_output["outcome"], "abort");

        // Idempotent: a second call after teardown is a no-op.
        c.complete_finished_session();
        assert!(c.session.is_none());
    }

    #[test]
    fn end_session_save_prefers_chosen_save_path() {
        let dir = std::env::temp_dir().join(format!("shot-chosen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = ScreenshotController::new();
        let mut s = session();
        s.test_select(Rect::new(0.0, 0.0, 10.0, 10.0), Some(0));
        s.chosen_save_path = Some(dir.to_string_lossy().to_string());
        c.set_session(s);
        c.end_session(
            ShotOutcome::Save,
            Some(ShotImage::with_png(10, 10, b"PNGDATA".to_vec())),
            None,
        );
        let path = c.last_output["path"].as_str().unwrap().to_string();
        assert!(path.starts_with(dir.to_str().unwrap()), "saved into the chosen dir: {path}");
        assert!(std::path::Path::new(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ocr_degrades_to_no_text() {
        // Not a decodable image, no bytes, and (off-main) any real image all
        // degrade to the empty string — never an error or a panic.
        let bad = ShotImage::with_png(10, 10, b"not-an-image".to_vec());
        assert_eq!(ocr_text(&bad, &ShotOCRConfigModel::default()), "");
        let none = ShotImage::new(10, 10);
        assert_eq!(ocr_text(&none, &ShotOCRConfigModel::default()), "");
    }

    #[test]
    fn deliver_text_lands_on_the_no_text_path() {
        let mut c = ScreenshotController::new();
        let cfg = ScreenshotConfig::default();
        let img = ShotImage::with_png(20, 10, b"not-an-image".to_vec());
        c.deliver_text(&img, &cfg, &ShotArgs::default());
        assert_eq!(c.last_output["outcome"], "text");
        assert_eq!(c.last_output["size"], json!([20, 10]));
        assert_eq!(c.last_output["chars"], 0);
        assert_eq!(c.last_output["text"], "");
        assert!(c.last_output["ms"].is_number(), "timing is always present");
        assert_eq!(c.toast.as_ref().unwrap().text, "No text found");
        assert_eq!(c.reply, Some(Vec::new()), "non-raw reply is empty");
    }

    #[test]
    fn wheel_geometry_and_hit_test() {
        assert_eq!(ShotWheel::radius(0), 56.0, "empty wheel keeps the minimum");
        assert_eq!(ShotWheel::radius(8), 56.0, "8 swatches still at the floor");
        assert!(ShotWheel::radius(20) > 56.0, "20 swatches grow the ring");

        let c = Point::new(100.0, 100.0);
        let first = ShotWheel::dot_center(c, 0, 4);
        assert!((first.x - 100.0).abs() < 1e-9, "index 0 is straight up");
        assert!(first.y < 100.0, "12 o'clock is above the centre");
        assert_eq!(ShotWheel::index_at(first, c, 4), Some(0), "swatch hits");
        assert_eq!(ShotWheel::index_at(c, c, 4), None, "the hub is empty");
        assert_eq!(ShotWheel::index_at(c, c, 0), None, "empty wheel never hits");
    }

    #[test]
    fn loupe_origin_flips_at_the_edges() {
        let size = Size::new(ShotLoupe::SIDE, ShotLoupe::SIDE + 22.0);
        let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let mid = ShotLoupe::origin(Point::new(200.0, 200.0), size, bounds);
        assert_eq!((mid.x, mid.y), (224.0, 224.0), "clear space: down-right");
        let right = ShotLoupe::origin(Point::new(950.0, 200.0), size, bounds);
        assert!(right.x < 950.0, "overflow right flips left");
        let bottom = ShotLoupe::origin(Point::new(200.0, 780.0), size, bounds);
        assert!(bottom.y < 780.0, "overflow bottom flips up");
    }

    #[test]
    fn chrome_layout_constants() {
        let pill = Rect::new(100.0, 40.0, 220.0, ShotModePill::HEIGHT);
        assert_eq!(
            ShotRecentButton::origin(pill),
            Point::new(332.0, 43.0),
            "right of and just below the pill"
        );
        assert!(ShotModePill::right_segment(230.0, 200.0), "second segment");
        assert!(!ShotModePill::right_segment(100.0, 200.0), "first segment");
        assert_eq!(ShotToolTab::origin_y(900.0, 50.0), 425.0, "centred vertically");
        assert_eq!(ShotSizeIndicator::FRAME, Rect::new(20.0, 20.0, 56.0, 44.0));
        assert_eq!(ShotSidePanel::list_height(800.0, 400.0), 350.0);
        assert_eq!(ShotSidePanel::list_height(100.0, 80.0), 80.0, "floor of 80");
    }

    #[test]
    fn help_card_size_and_origin() {
        assert_eq!(ShotHelpCard::height_for(3, 17.0), 83.0, "17*3 + 32");
        assert_eq!(ShotHelpCard::height(0), 32.0, "empty card is just padding");
        assert_eq!(
            ShotHelpCard::centered_origin(
                Rect::new(0.0, 0.0, 1000.0, 800.0),
                Size::new(300.0, 100.0),
            ),
            Point::new(350.0, 350.0),
            "centred"
        );
    }

    #[test]
    fn save_card_focus_selects_the_file_stem() {
        assert_eq!(
            ShotSaveCard::focus_range("~/Desktop/shot-1.png"),
            (10, 6),
            "selects the name without the extension"
        );
        assert_eq!(
            ShotSaveCard::focus_range("/tmp/a/b/shot-1.jpeg"),
            (9, 6),
            "last path component only"
        );
        assert_eq!(ShotSaveCard::focus_range("name"), (0, 4), "no extension");
    }
}
