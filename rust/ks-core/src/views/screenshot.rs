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
//! Real: the side panel (Space / the Tool Settings tab) — Flameshot's sidebar
//! painted into the overlay context like the rest of the chrome: tool size
//! (label + spin box + slider), active colour, Grab Color, a hue/saturation
//! wheel, the hex field (edited through the key router) and Display grid.
//! Layout, hit-test and value mapping live in `ShotSidePanel`. Not yet: text
//! options (G3) and the layers list (G5) — see BACKLOG.md "Screenshot".
//! The save-card field is edited through the key router (no AppKit control) and
//! the ring buttons' SF Symbols are drawn by `draw_symbol`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app::registry::Registry;
use crate::engines::screenshot_annotations::{
    round_half_away, ButtonRing, Point, Rect, ShotArgs, ShotColor, ShotDate, ShotDocument,
    ShotFiles, ShotGeom, ShotMode, ShotObject, ShotSnap, ShotState, ShotTextStyle, ShotTool, Size,
};

/// The frozen base image behind a [`ShotDisplay`] (macOS only; `()` elsewhere).
#[cfg(target_os = "macos")]
pub type BaseImage = objc2::rc::Retained<objc2_core_graphics::CGImage>;

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

/// Port of Swift `initialRegion` (~590–607): resolve `--region` / `--last-region`
/// to a `(local rect, display index)`. `screenN` names a display; otherwise a
/// global `WxH+X+Y` is offset into the first display whose local bounds contain
/// its top-left; `--last-region` matches the saved region by display id.
/// `[screenshot] merge-*`: how Recents ▸ Shift-click stacks screenshots.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeConfig {
    /// Space between stacked screenshots (points).
    pub gap: f64,
    /// The left column that holds each screenshot's number (points).
    pub gutter: f64,
    /// The number labels' font size (points).
    pub label_size: f64,
    /// Fill behind and between the screenshots.
    pub background: ShotColor,
    /// The number discs (an editable Counter per screenshot; the number
    /// itself is drawn black or white for contrast).
    pub label_color: ShotColor,
    /// The line around each screenshot (points; 0 = none) and its colour.
    pub border_width: f64,
    pub border_color: ShotColor,
    /// Most pixels per point the merged editor canvas may use. A stack too
    /// tall to fit the screen at native resolution within this is scaled down.
    pub max_scale: f64,
    /// Recents header hint while nothing is picked ("" hides it).
    pub hint: String,
    /// Recents header hint while picking (`{}` = how many).
    pub picked_hint: String,
    /// The stacking the Recents pill starts on (`merge-direction`).
    pub horizontal: bool,
    /// The Recents pill's two labels.
    pub vertical_label: String,
    pub horizontal_label: String,
    /// File lists ▸ right-click on one picture (`editor-menu-title`) and on
    /// 2+ pictures (`editor-merge-menu-title`, `{}` = how many).
    pub editor_menu_title: String,
    pub editor_merge_menu_title: String,
    /// The merge editor's reorder strip header (`merge-order-hint`).
    pub order_hint: String,
}

impl Default for MergeConfig {
    fn default() -> Self {
        MergeConfig {
            gap: 16.0,
            gutter: 44.0,
            label_size: 18.0,
            background: ShotColor::new_a(0x1e as f64 / 255.0, 0x1e as f64 / 255.0, 0x1e as f64 / 255.0, 1.0),
            label_color: ShotColor::from_hex("#ffc400").unwrap_or(ShotColor::WHITE),
            border_width: 1.0,
            border_color: ShotColor::from_hex("#8c8c8c").unwrap_or(ShotColor::WHITE),
            max_scale: 3.0,
            hint: "Shift-click to merge".to_string(),
            picked_hint: "{} picked \u{b7} release Shift to merge".to_string(),
            horizontal: false,
            vertical_label: "\u{2193} Vertical".to_string(),
            horizontal_label: "\u{2192} Horizontal".to_string(),
            editor_menu_title: "Open in Screenshot Editor".to_string(),
            editor_merge_menu_title: "Merge {} Photos in Screenshot Editor".to_string(),
            order_hint: "Drag to reorder".to_string(),
        }
    }
}

/// Where each screenshot and its number go in a merged stack (pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct MergeLayout {
    pub width: f64,
    pub height: f64,
    /// One per screenshot, top to bottom, in pick order.
    pub images: Vec<Rect>,
    /// The number cell left of each image (gutter width × image height).
    pub labels: Vec<Rect>,
}

/// Stack `sizes` (pixels): top to bottom (`horizontal = false`) or left to
/// right. Vertical: the widest screenshot sets the column width, narrower
/// ones sit left-aligned, each row keeps its own height, and the numbers go
/// in a `gutter`-wide column on the left. Horizontal: the tallest sets the
/// row height, shorter ones sit top-aligned, each keeps its own width, and
/// the numbers go in a `gutter`-high strip above. Never upscaled (screenshots
/// stay sharp); the background fills the rest. All values are pixels.
pub fn merge_layout(sizes: &[(f64, f64)], horizontal: bool, gutter: f64, gap: f64, pad: f64) -> MergeLayout {
    if horizontal {
        let row_h = sizes.iter().map(|s| s.1).fold(0.0, f64::max);
        let mut x = pad;
        let mut images = Vec::new();
        let mut labels = Vec::new();
        for (i, &(w, h)) in sizes.iter().enumerate() {
            if i > 0 {
                x += gap;
            }
            labels.push(Rect::new(x, pad, w, gutter));
            images.push(Rect::new(x, pad + gutter, w, h));
            x += w;
        }
        return MergeLayout { width: x + pad, height: pad + gutter + row_h + pad, images, labels };
    }
    let col_w = sizes.iter().map(|s| s.0).fold(0.0, f64::max);
    let mut y = pad;
    let mut images = Vec::new();
    let mut labels = Vec::new();
    for (i, &(w, h)) in sizes.iter().enumerate() {
        if i > 0 {
            y += gap;
        }
        labels.push(Rect::new(pad, y, gutter, h));
        images.push(Rect::new(pad + gutter, y, w, h));
        y += h;
    }
    MergeLayout {
        width: pad + gutter + col_w + pad,
        height: y + pad,
        images,
        labels,
    }
}

/// How the merged stack sits on the editor canvas for one display.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeStage {
    /// Canvas pixels per point (the editor's output resolution).
    pub scale: f64,
    /// Merged-stack pixels → canvas pixels (1 = native, < 1 = shrunk to fit).
    pub fit: f64,
    /// Where the stack sits on the display, in points (top-left origin):
    /// the editor's preselected region.
    pub rect: Rect,
}

/// Fit a `w`×`h` px stack on a display of `frame_w`×`frame_h` points (minus
/// `top_inset` for the menu bar and `margin` all round). Native pixels when
/// that fits within `max_scale` px/pt (never below the display's own
/// `min_scale`); otherwise the stack is shrunk to fit.
pub fn merge_stage(
    w: f64,
    h: f64,
    frame_w: f64,
    frame_h: f64,
    top_inset: f64,
    margin: f64,
    min_scale: f64,
    max_scale: f64,
) -> MergeStage {
    merge_stage_left(w, h, frame_w, frame_h, top_inset, 0.0, margin, min_scale, max_scale)
}

/// `merge_stage` with `left` points kept free on the left edge (the
/// reorder strip); the stack is centred in what is left.
#[allow(clippy::too_many_arguments)]
pub fn merge_stage_left(
    w: f64,
    h: f64,
    frame_w: f64,
    frame_h: f64,
    top_inset: f64,
    left: f64,
    margin: f64,
    min_scale: f64,
    max_scale: f64,
) -> MergeStage {
    let avail_w = (frame_w - left - 2.0 * margin).max(1.0);
    let avail_h = (frame_h - top_inset - 2.0 * margin).max(1.0);
    let need = (w / avail_w).max(h / avail_h).max(f64::MIN_POSITIVE);
    let scale = need.clamp(min_scale.max(1.0), max_scale.max(min_scale).max(1.0));
    let fit = (scale / need).min(1.0);
    let (rw, rh) = (w * fit / scale, h * fit / scale);
    // Whole canvas pixels, so the native case copies pixels 1:1.
    let snap = |v: f64| (v * scale).round() / scale;
    let x = snap(left + margin + (avail_w - rw) / 2.0);
    let y = snap(top_inset + margin + (avail_h - rh) / 2.0);
    MergeStage { scale, fit, rect: Rect::new(x, y, rw, rh) }
}

/// One fixed-number Counter per merged screenshot, at the top of its gutter
/// cell, in session points (`px` = screenshot pixels per gutter point).
pub fn merge_labels(stage: &MergeStage, layout: &MergeLayout, m: &MergeConfig, px: f64, outline: bool) -> Vec<ShotObject> {
    let to_pt = |v: f64| v * stage.fit / stage.scale;
    let gutter_pt = to_pt(m.gutter * px);
    let r = (m.label_size * 0.85).min(gutter_pt * 0.45).max(12.0);
    layout
        .labels
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            // The disc sits in the cell's top-left gutter-sized square: the
            // left column (vertical) or the strip above (horizontal).
            let c = Point::new(
                stage.rect.x + to_pt(cell.x) + gutter_pt / 2.0,
                stage.rect.y + to_pt(cell.y) + gutter_pt / 2.0,
            );
            let mut o = ShotObject::new(ShotTool::Counter, vec![c], m.label_color, (r - 12.0).round() as i64);
            o.number = i as i64 + 1;
            o.fixed_number = true;
            o.outline = outline;
            o
        })
        .collect()
}

/// Move the item at `from` to `to` (remove + insert); the items between
/// shift by one. Out-of-range indices are a no-op.
pub fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if from < v.len() && to < v.len() && from != to {
        let x = v.remove(from);
        v.insert(to, x);
    }
}

/// The slot order the reorder strip shows while row `from` is dragged over
/// slot `to`: `order[slot]` = the photo (current stack index) drawn there.
pub fn merge_preview_order(n: usize, drag: Option<(usize, usize)>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    if let Some((from, to)) = drag {
        move_item(&mut order, from, to);
    }
    order
}

/// The merge editor's reorder strip rects (display points, top-left origin).
#[derive(Clone, Debug, PartialEq)]
pub struct MergeStripLayout {
    pub strip: Rect,
    pub hint: Rect,
    /// The stacking toggle: [vertical, horizontal].
    pub direction: [Rect; 2],
    /// One row per slot, top to bottom (slot 0 = photo 1).
    pub rows: Vec<Rect>,
}

/// `MergeStrip` — the numbered thumbnail list left of a merged stack. Drag a
/// row up or down to move that photo; the numbers follow the new order.
pub struct MergeStrip;

impl MergeStrip {
    pub const WIDTH: f64 = 156.0;
    /// The ↓ Vertical | → Horizontal toggle's height.
    pub const TOGGLE: f64 = 24.0;
    pub const PAD: f64 = 8.0;
    pub const HINT: f64 = 22.0;
    pub const ROW: f64 = 84.0;
    pub const MIN_ROW: f64 = 28.0;
    pub const GAP: f64 = 6.0;
    /// Space between the strip and the screen edge / Tool Settings tab.
    pub const INSET: f64 = 8.0;

    /// Horizontal room a merged stack leaves free for the strip (and the tab).
    pub fn reserve() -> f64 {
        ShotToolTab::WIDTH + Self::INSET + Self::WIDTH + Self::INSET
    }

    /// The strip for `n` photos on a display `bounds` (below `top`, the
    /// menu-bar inset), its left edge at `x0`, centred vertically. Rows
    /// shrink (down to `MIN_ROW`) so all `n` fit.
    pub fn layout(bounds: Rect, top: f64, x0: f64, n: usize) -> Option<MergeStripLayout> {
        if n < 2 {
            return None;
        }
        let avail = (bounds.h - top - 2.0 * Self::INSET).max(1.0);
        let chrome = 2.0 * Self::PAD + Self::HINT + Self::TOGGLE + Self::GAP;
        let per = ((avail - chrome + Self::GAP) / n as f64 - Self::GAP).floor();
        let row = per.clamp(Self::MIN_ROW, Self::ROW);
        let h = chrome + n as f64 * row + (n as f64 - 1.0) * Self::GAP;
        let y = (bounds.y + top + Self::INSET + ((avail - h) / 2.0).max(0.0)).round();
        let strip = Rect::new(x0, y, Self::WIDTH, h);
        let iw = Self::WIDTH - 2.0 * Self::PAD;
        let hint = Rect::new(x0 + Self::PAD, y + Self::PAD, iw, Self::HINT);
        let half = (iw / 2.0).floor();
        let direction = [
            Rect::new(x0 + Self::PAD, hint.max_y(), half, Self::TOGGLE),
            Rect::new(x0 + Self::PAD + half, hint.max_y(), iw - half, Self::TOGGLE),
        ];
        let top = hint.max_y() + Self::TOGGLE + Self::GAP;
        let rows = (0..n)
            .map(|i| {
                Rect::new(
                    x0 + Self::PAD,
                    top + i as f64 * (row + Self::GAP),
                    Self::WIDTH - 2.0 * Self::PAD,
                    row,
                )
            })
            .collect();
        Some(MergeStripLayout { strip, hint, direction, rows })
    }

    /// The row under `p`.
    pub fn row_at(l: &MergeStripLayout, p: Point) -> Option<usize> {
        l.rows.iter().position(|r| r.contains_point(p))
    }

    /// The slot a drag at height `y` drops into (clamped to the list).
    pub fn slot_at(l: &MergeStripLayout, y: f64) -> usize {
        let n = l.rows.len();
        if n == 0 {
            return 0;
        }
        l.rows
            .iter()
            .position(|r| y < r.max_y() + Self::GAP / 2.0)
            .unwrap_or(n - 1)
    }
}

/// A merged stack open in the editor: what the reorder strip needs to
/// rebuild the canvas in a new order.
#[cfg(target_os = "macos")]
pub struct MergeEdit {
    /// The photos in stack order (unreadable files already dropped).
    pub paths: Vec<String>,
    pub images: Vec<BaseImage>,
    pub horizontal: bool,
    /// Screenshot pixels per gutter point (the stack is built in pixels).
    pub px: f64,
    pub layout: MergeLayout,
    pub stage: MergeStage,
    /// The canvas size in pixels.
    pub canvas: (i64, i64),
    /// The display the editor is on.
    pub display: usize,
    /// A live strip drag: (row picked up, slot it is over).
    pub drag: Option<(usize, usize)>,
}

/// Put each captured display image into the session slot for that display.
#[cfg(target_os = "macos")]
fn set_session_images(s: &mut ShotSession, images: &[CapturedImage]) {
    for image in images {
        if let Some(i) = s.displays.iter().position(|d| d.id == image.display) {
            s.set_base_image(i, image.image.clone());
        }
    }
}

/// A capture result carried from the capture thread back to main.
#[cfg(target_os = "macos")]
struct SendCapture(Result<Vec<CapturedImage>, CaptureError>);
// SAFETY: the CGImages inside are immutable once created and only read on main.
#[cfg(target_os = "macos")]
unsafe impl Send for SendCapture {}

pub fn initial_region(args: &ShotArgs, s: &ShotSession) -> Option<(Rect, usize)> {
    if let Some(reg) = args.region.as_deref() {
        if let Some(rest) = reg.strip_prefix("screen") {
            if let Ok(n) = rest.parse::<usize>() {
                if n < s.displays.len() {
                    return Some((s.displays[n].bounds(), n));
                }
            }
        }
        if let Some(g) = ShotArgs::parse_region(reg) {
            for (i, d) in s.displays.iter().enumerate() {
                let o = s.global_origin(d);
                let local = Rect::new(g.x - o.x, g.y - o.y, g.w, g.h);
                if d.bounds().contains_point(Point::new(local.x, local.y)) {
                    return Some((local, i));
                }
            }
        }
    }
    if args.last_region {
        if let Some(l) = s.state.last_region.as_ref() {
            if let Some(i) = s.displays.iter().position(|d| d.id == l.display) {
                return Some((Rect::new(l.x, l.y, l.w, l.h), i));
            }
        }
    }
    None
}

/// Encode a captured [`CGImage`] as a [`ShotImage`] (macOS only).
#[cfg(target_os = "macos")]
fn encode_shot_image(image: &objc2_core_graphics::CGImage) -> Option<ShotImage> {
    let w = objc2_core_graphics::CGImage::width(Some(image)) as i64;
    let h = objc2_core_graphics::CGImage::height(Some(image)) as i64;
    shot_cg::encode_png(image).map(|png| ShotImage::with_png(w, h, png))
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

pub const DEFAULT_USER_COLORS: &str =
    "picker, #800000, #ff0000, #ffff00, #00ff00, #008000, #00ffff, #0000ff, #ff00ff, #800080";

/// Every overlay shortcut, in one table: `(config key, key label, default
/// text, on the help card)`. The help card (shown until a selection exists)
/// takes the `true` rows; Cmd+/ shows them all. Each text is overridable with
/// its `[screenshot] help-*` key (rule 3).
pub const SHORTCUT_KEYS: [(&str, &str, &str, bool); 23] = [
    ("help-mouse", "Mouse", "Select an area \u{b7} W snaps to a window", true),
    ("help-copy", "\u{2318}C / Return", "Copy to clipboard", true),
    ("help-save", "\u{2318}S", "Save to a file", true),
    ("help-pin", "\u{2318}P", "Pin on screen", true),
    ("help-copy-text", "Tab", "Copy text mode", true),
    ("help-record", "V", "Record the selection (Hyper+X stops)", true),
    ("help-last-region", "L", "Select the last area again", true),
    ("help-recent", "\u{2318}R", "Recent screenshots", true),
    ("help-space", "Space", "Tool settings", true),
    ("help-shortcuts", "\u{2318}/", "All shortcuts", true),
    ("help-esc", "Esc", "Exit", true),
    ("help-tools", "P D A R F C H T B I", "Pencil, line, arrow, rectangle, filled, circle, highlighter, text, pixelate, invert", false),
    ("help-retake", "\u{2318}T", "Retake after the delay (menus, hovers)", false),
    ("help-copy-text-now", "\u{21e7}\u{2318}C", "Copy the text in the selection", false),
    ("help-wheel", "Wheel / [ ]", "Change tool or font size", false),
    ("help-text-style", "\u{2318}B / \u{2318}I / \u{2318}\u{00b1}", "Bold, italic, or resize text while typing", false),
    ("help-right-click", "Right Click", "Colour wheel", false),
    ("help-grab", "G / \u{21e7}G", "Pick a colour from the screen (\u{21e7}G also copies its hex)", false),
    ("help-arrows", "Arrows", "Move the selection (\u{21e7} resizes)", false),
    ("help-move", "\u{2318}M", "Move mode", false),
    ("help-undo", "\u{2318}Z / \u{21e7}\u{2318}Z", "Undo / redo", false),
    ("help-select-all", "\u{2318}A", "Select the whole screen", false),
    ("help-delete", "Delete", "Delete the selected drawing", false),
];

pub const HELP_TEXT_KEYS: [(&str, &str, &str); 4] = [
    ("help-text", "Mouse", "Drag over text to copy it"),
    ("help-text-mode", "Tab", "Back to screenshot mode"),
    ("help-recent", "\u{2318}R", "Recent screenshots"),
    ("help-esc", "Esc", "Exit"),
];

/// `(label, text)` rows from [`SHORTCUT_KEYS`]: the help card's (`card_only`)
/// or all of them, each text through `text(config key, default)`.
pub fn shortcut_rows(card_only: bool, text: impl Fn(&str, &str) -> String) -> Vec<(String, String)> {
    SHORTCUT_KEYS
        .iter()
        .filter(|(_, _, _, card)| *card || !card_only)
        .map(|(key, label, def, _)| (label.to_string(), text(key, def)))
        .collect()
}

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
    /// `[screenshot] text-accuracy = accurate` (default `fast`; see
    /// `ShotOCRConfig::accurate`).
    pub accurate: bool,
    /// `[screenshot] text-column-gap`: blocks on one row further apart than
    /// this × the line height are joined with a tab (0 = a space).
    pub column_gap: f64,
    /// `[screenshot] text-barcodes`: a QR / barcode in the area is copied as
    /// its payload (a URL, a Wi-Fi string …) instead of the text around it.
    pub barcodes: bool,
    /// `text-accuracy = auto` (the default): `.accurate` once loaded.
    pub auto_accurate: bool,
}

impl Default for ShotOCRConfigModel {
    fn default() -> Self {
        ShotOCRConfigModel {
            languages: Vec::new(),
            correction: true,
            accurate: false,
            column_gap: 0.0,
            barcodes: true,
            auto_accurate: false,
        }
    }
}

/// The side panel's user-facing strings (`[screenshot] side-panel-*`).
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLabels {
    pub size: String,
    pub color: String,
    pub grab: String,
    pub grid: String,
    pub no_tool: String,
    pub tab: String,
    pub text: String,
    pub layers: String,
}

impl Default for PanelLabels {
    fn default() -> Self {
        PanelLabels {
            size: "Active tool size:".to_string(),
            color: "Active Color:".to_string(),
            grab: "Grab Color".to_string(),
            grid: "Display grid".to_string(),
            no_tool: "Pick a tool".to_string(),
            tab: "Tool Settings".to_string(),
            text: "Text:".to_string(),
            layers: "Layers".to_string(),
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
    pub highlighter_color: ShotColor,
    pub user_colors: Vec<Option<ShotColor>>,
    pub buttons: Vec<ShotTool>,
    pub button_size: f64,
    pub show_help: bool,
    /// `[screenshot] help-opacity` (0.0…1.0): translucency of the centered
    /// blue/purple help card before a selection exists (also auto-fades on hover).
    pub help_opacity: f64,
    pub show_side_panel_button: bool,
    /// `[screenshot] side-panel-width` (Flameshot's sidebar is 250pt).
    pub side_panel_width: f64,
    /// Side-panel labels: size, colour, grab, grid, no-tool hint, tab.
    pub panel_labels: PanelLabels,
    /// `draw-thickness` / `draw-font-size`: the starting size for line tools
    /// and the text tool until a per-tool size is remembered (0 = built-in).
    pub draw_thickness: i64,
    pub draw_font_size: i64,
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
    /// Where every capture is kept for Recents (`history-dir`, `~` allowed);
    /// the Files / Find Recent Files sidebar pins it as "Screenshots".
    pub history_dir: String,
    pub copy_path_after_save: bool,
    pub preview: bool,
    pub undo_limit: i64,
    /// Recents ▸ Shift-click merge (`[screenshot] merge-*`).
    pub merge: MergeConfig,
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
    /// Title when Copy Text found QR / barcodes (`{}` = "a QR code", "2 codes").
    pub code_toast: String,
    /// Shift+G (pick a colour): also copy its hex (`grab-copies-hex`; plain
    /// G never touches the clipboard), with this toast (`hex-toast`).
    pub grab_copies_hex: bool,
    pub hex_toast: String,
    pub no_text_toast: String,
    pub help_text_rows: Vec<(String, String)>,
    pub shortcuts: Vec<(String, String)>,
    /// Cmd+T: hide the overlay, wait this long, capture the same area again
    /// (`[screenshot] retake-delay`, seconds; for menus and hover states).
    pub retake_delay_ms: i64,
    /// Files ▸ right-click on pictures (`text-menu-title`; empty = no row).
    pub text_menu_title: String,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        ScreenshotConfig {
            enabled: true,
            ui_color: hex("#740096"),
            contrast_color: hex("#270032"),
            contrast_opacity: 190,
            draw_color: hex("#ff0000"),
            highlighter_color: hex("#ffe600"),
            user_colors: vec![None],
            buttons: ShotTool::ring("", false),
            button_size: 34.0,
            show_help: true,
            help_opacity: 0.45,
            show_side_panel_button: true,
            side_panel_width: ShotSidePanel::WIDTH,
            panel_labels: PanelLabels::default(),
            draw_thickness: 0,
            draw_font_size: 0,
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
            history_dir: DEFAULT_HISTORY_DIR.to_string(),
            copy_path_after_save: false,
            preview: true,
            undo_limit: 100,
            merge: MergeConfig::default(),
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
            help_rows: shortcut_rows(true, |_, d| d.to_string()),
            start_text: false,
            ocr: ShotOCRConfigModel::default(),
            text_toast: "Copied {} to clipboard".to_string(),
            code_toast: "Copied the contents of {}".to_string(),
            grab_copies_hex: true,
            hex_toast: "Copied {}".to_string(),
            no_text_toast: "No text found".to_string(),
            help_text_rows: HELP_TEXT_KEYS
                .iter()
                .map(|(_, label, text)| (label.to_string(), text.to_string()))
                .collect(),
            shortcuts: shortcut_rows(false, |_, d| d.to_string()),
            retake_delay_ms: 3000,
            text_menu_title: "Copy Text from Picture".to_string(),
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
            highlighter_color: col("highlighter-color", d.highlighter_color),
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

        c.buttons = ShotTool::ring(&s("buttons", ""), b("show-size-badge", false));
        let bs = i("button-size", 0, 0, 80);
        c.button_size = if bs >= 20 {
            bs as f64
        } else {
            ButtonRing::default_button_size(15.5)
        };
        c.show_help = b("show-help", true);
        if let Some(op) = e.get("help-opacity").and_then(|v| v.trim().parse::<f64>().ok()) {
            c.help_opacity = op.clamp(0.0, 1.0);
        }
        c.show_side_panel_button = b("show-side-panel-button", true);
        c.side_panel_width = i("side-panel-width", ShotSidePanel::WIDTH as i64, 180, 480) as f64;
        c.panel_labels = PanelLabels {
            size: s("side-panel-size-label", &d.panel_labels.size),
            color: s("side-panel-color-label", &d.panel_labels.color),
            grab: s("side-panel-grab-label", &d.panel_labels.grab),
            grid: s("side-panel-grid-label", &d.panel_labels.grid),
            no_tool: s("side-panel-no-tool", &d.panel_labels.no_tool),
            tab: s("side-panel-tab-label", &d.panel_labels.tab),
            text: s("side-panel-text-label", &d.panel_labels.text),
            layers: s("side-panel-layers-label", &d.panel_labels.layers),
        };
        c.draw_thickness = i("draw-thickness", 0, 0, 100);
        c.draw_font_size = i("draw-font-size", 0, 0, 100);
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
        c.history_dir = s("history-dir", DEFAULT_HISTORY_DIR);
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
        c.undo_limit = i("undo-limit", 100, 1, 1000);
        let md = MergeConfig::default();
        c.merge = MergeConfig {
            gap: i("merge-gap", md.gap as i64, 0, 200) as f64,
            gutter: i("merge-gutter", md.gutter as i64, 0, 200) as f64,
            label_size: i("merge-label-size", md.label_size as i64, 8, 96) as f64,
            background: col("merge-background", md.background),
            label_color: col("merge-label-color", md.label_color),
            border_width: i("merge-border-width", md.border_width as i64, 0, 20) as f64,
            border_color: col("merge-border-color", md.border_color),
            max_scale: i("merge-max-scale", md.max_scale as i64, 1, 6) as f64,
            hint: e.get("merge-hint").cloned().unwrap_or(md.hint),
            picked_hint: s("merge-picked-hint", &md.picked_hint),
            horizontal: s("merge-direction", "vertical").to_lowercase().starts_with('h'),
            vertical_label: s("merge-vertical-label", &md.vertical_label),
            horizontal_label: s("merge-horizontal-label", &md.horizontal_label),
            editor_menu_title: s("editor-menu-title", &md.editor_menu_title),
            editor_merge_menu_title: s("editor-merge-menu-title", &md.editor_merge_menu_title),
            order_hint: e.get("merge-order-hint").cloned().unwrap_or(md.order_hint),
        };
        c.arrow_style = i("arrow-style", 0, 0, 1);
        c.reverse_arrow = b("reverse-arrow", false);
        c.counter_outline = b("counter-outline", true);
        c.insecure_pixelate = b("insecure-pixelate", false);
        c.delay_ms = i("delay", 0, 0, 60_000);
        c.retake_delay_ms = i("retake-delay", 3, 1, 30) * 1000;
        c.text_menu_title = s("text-menu-title", &d.text_menu_title);
        c.font = e.get("font").cloned().unwrap_or_default();
        if let Some(v) = e.get("copy-toast") {
            c.copy_toast = v.clone();
        }
        if let Some(v) = e.get("save-toast") {
            c.save_toast = v.clone();
        }
        c.permission_toast = s("permission-toast", &d.permission_toast);
        c.fail_toast = s("fail-toast", &d.fail_toast);
        c.help_rows = shortcut_rows(true, |k, d| s(k, d));
        c.shortcuts = shortcut_rows(false, |k, d| s(k, d));
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
        c.ocr.barcodes = b("text-barcodes", true);
        c.code_toast = s("code-toast", &d.code_toast);
        c.grab_copies_hex = b("grab-copies-hex", true);
        c.hex_toast = s("hex-toast", &d.hex_toast);
        // fast | accurate | auto (default: fast until the accurate model has
        // loaded in the background, then accurate).
        let acc = s("text-accuracy", "fast").to_lowercase();
        c.ocr.accurate = acc == "accurate";
        c.ocr.auto_accurate = acc == "auto";
        c.ocr.column_gap = e
            .get("text-column-gap")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|g| g.is_finite() && *g >= 0.0)
            .unwrap_or(0.0);
        if let Some(v) = e.get("text-toast") {
            c.text_toast = v.clone();
        }
        c.no_text_toast = s("no-text-toast", &d.no_text_toast);
        c
    }

}

/// One on-screen window for the W (snap to window) key: its bounds in global
/// top-left coordinates, its layer and its owner's pid, front-most first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenWindow {
    pub bounds: Rect,
    pub layer: i64,
    pub pid: i32,
}

/// The front-most ordinary app window (layer 0, not ours, not a sliver)
/// under the global point `at`.
pub fn window_under(windows: &[ScreenWindow], at: Point, own_pid: i32) -> Option<Rect> {
    windows
        .iter()
        .find(|w| {
            w.layer == 0
                && w.pid != own_pid
                && w.bounds.w >= 40.0
                && w.bounds.h >= 40.0
                && at.x >= w.bounds.x
                && at.x <= w.bounds.x + w.bounds.w
                && at.y >= w.bounds.y
                && at.y <= w.bounds.y + w.bounds.h
        })
        .map(|w| w.bounds)
}

/// `CGWindowListCopyWindowInfo(onScreenOnly | excludeDesktop)`, front to back.
#[cfg(target_os = "macos")]
pub fn on_screen_windows() -> Vec<ScreenWindow> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut core::ffi::c_void;
    }
    let raw = unsafe { CGWindowListCopyWindowInfo(1 | (1 << 4), 0) };
    if raw.is_null() {
        return Vec::new();
    }
    // CFArray of CFDictionary is toll-free bridged to NSArray<NSDictionary>;
    // the Copy call returned it +1.
    let Some(arr) = (unsafe { Retained::<NSArray<NSDictionary<NSString, AnyObject>>>::from_raw(raw.cast()) }) else {
        return Vec::new();
    };
    let num = |d: &NSDictionary<NSString, AnyObject>, k: &str| -> Option<f64> {
        let v = d.objectForKey(&NSString::from_str(k))?;
        let n: &NSNumber = unsafe { &*(Retained::as_ptr(&v) as *const NSNumber) };
        Some(n.doubleValue())
    };
    let mut out = Vec::new();
    for d in arr.iter() {
        let layer = num(&d, "kCGWindowLayer").unwrap_or(-1.0) as i64;
        let pid = num(&d, "kCGWindowOwnerPID").unwrap_or(0.0) as i32;
        let Some(b) = d.objectForKey(&NSString::from_str("kCGWindowBounds")) else { continue };
        let bd: &NSDictionary<NSString, AnyObject> = unsafe { &*(Retained::as_ptr(&b) as *const NSDictionary<NSString, AnyObject>) };
        let (Some(x), Some(y), Some(w), Some(h)) = (num(bd, "X"), num(bd, "Y"), num(bd, "Width"), num(bd, "Height")) else {
            continue;
        };
        out.push(ScreenWindow { bounds: Rect::new(x, y, w, h), layer, pid });
    }
    out
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
    /// Menu bar / notch height (`frame.maxY - visibleFrame.maxY`).
    pub top_inset: f64,
}

impl ScreenRef {
    pub fn new(id: u32, frame: Rect, scale: f64) -> Self {
        ScreenRef { id, frame, scale, top_inset: 0.0 }
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
    /// The real ScreenCaptureKit backend: the interactive overlay may go up
    /// first and capture behind it (see `start_async_session`).
    fn live(&self) -> bool {
        false
    }
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
    fn live(&self) -> bool {
        true
    }
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

    /// The `SCDisplay` list from the last `SCShareableContent` fetch. A fetch
    /// enumerates every on-screen window through the WindowServer (15–20 ms
    /// warm, 40+ ms cold) and displays almost never change, so the capture
    /// path reuses this list. It is fetched again when a screen id is missing
    /// (display plugged in / swapped) or when a capture with it fails.
    struct DisplayCache(Retained<NSArray<SCDisplay>>);
    // SAFETY: `SCDisplay` is an immutable descriptor (id + frame); ScreenCaptureKit
    // hands these out on its own queues, and we only read them.
    unsafe impl Send for DisplayCache {}
    static DISPLAYS: std::sync::Mutex<Option<DisplayCache>> = std::sync::Mutex::new(None);

    fn fresh_displays() -> Result<Retained<NSArray<SCDisplay>>, CaptureError> {
        let content = fetch_shareable_content()?;
        let displays = unsafe { content.displays() };
        if let Ok(mut g) = DISPLAYS.lock() {
            *g = Some(DisplayCache(displays.clone()));
        }
        Ok(displays)
    }

    fn cached_displays(screens: &[ScreenRef]) -> Result<Retained<NSArray<SCDisplay>>, CaptureError> {
        let cached = DISPLAYS.lock().ok().and_then(|g| g.as_ref().map(|c| c.0.clone()));
        if let Some(d) = cached {
            if screens.iter().all(|s| match_display(&d, s.id).is_some()) {
                return Ok(d);
            }
        }
        fresh_displays()
    }

    /// Fill the display cache and run one throwaway 16x16 capture ahead of the
    /// first Hyper+X: the first `SCScreenshotManager` capture in a process
    /// lands ~60–90 ms later than the rest (owner's logs: 105–140 ms vs ~47 ms
    /// once warm), and the display list alone does not pay that.
    /// Blocking; call off the main thread.
    pub fn prewarm() {
        let Ok(displays) = fresh_displays() else { return };
        let Some(display) = displays.iter().next() else { return };
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::<SCWindow>::new(),
            )
        };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(16);
            config.setHeight(16);
            config.setShowsCursor(false);
        }
        let (block, rx) = capture_block();
        unsafe {
            SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                &filter,
                &config,
                Some(&block),
            );
        }
        let _ = rx.recv_timeout(TIMEOUT);
    }

    pub fn capture_displays(screens: &[ScreenRef]) -> Result<Vec<CapturedImage>, CaptureError> {
        if screens.is_empty() {
            return Err(CaptureError::new("no screens to capture"));
        }
        let displays = cached_displays(screens)?;
        match capture_with(&displays, screens) {
            Ok(out) => Ok(out),
            // A stale descriptor (display reconfigured under the same id):
            // refetch once and retry.
            Err(_) => capture_with(&*fresh_displays()?, screens),
        }
    }

    fn capture_with(
        displays: &NSArray<SCDisplay>,
        screens: &[ScreenRef],
    ) -> Result<Vec<CapturedImage>, CaptureError> {
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

    /// Capture a single on-screen window by its `CGWindowID` (the AppKit
    /// `windowNumber`) through `SCContentFilter::initWithDesktopIndependentWindow`.
    /// The output is the window's own content at `scale` × its point size — no
    /// crop, no shadow, no overlay. Safe to call off the main thread.
    pub fn capture_window(window_id: u32, scale: f64) -> Result<CapturedImage, CaptureError> {
        let content = fetch_shareable_content()?;
        let windows = unsafe { content.windows() };
        let window = windows
            .iter()
            .find(|w| unsafe { w.windowID() } == window_id)
            .ok_or_else(|| CaptureError::new(format!("window {window_id} not found")))?;
        let filter = unsafe {
            SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
        };
        let config = unsafe { SCStreamConfiguration::new() };
        let frame = unsafe { window.frame() };
        let width = (frame.size.width * scale).round().max(1.0) as usize;
        let height = (frame.size.height * scale).round().max(1.0) as usize;
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
        let image = match rx.recv_timeout(TIMEOUT) {
            Ok(Ok(image)) => image,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(CaptureError::new("window-shot: capture timed out")),
        };
        Ok(CapturedImage {
            display: window_id,
            width: CGImage::width(Some(&image)) as i64,
            height: CGImage::height(Some(&image)) as i64,
            scale,
            image: Some(image),
        })
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// `NSScreen.screens` as [`ScreenRef`]s: the `NSScreenNumber` display id, the
/// AppKit frame (the overlay panels use it as-is) and the backing scale.
/// Empty off the main thread.
#[cfg(target_os = "macos")]
pub fn current_screens() -> Vec<ScreenRef> {
    crate::platform::screens()
        .into_iter()
        .map(|s| {
            let mut r = ScreenRef::new(
                s.id,
                Rect::new(s.frame.x, s.frame.y, s.frame.w, s.frame.h),
                s.scale,
            );
            r.top_inset = s.frame.y + s.frame.h - (s.visible.y + s.visible.h);
            r
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn screen_capture_permitted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Fill the ScreenCaptureKit display cache on a background thread so the first
/// Hyper+X skips the window enumeration. No-op without Screen Recording.
#[cfg(target_os = "macos")]
pub fn prewarm_capture() {
    if !screen_capture_permitted() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("ks-sck-prewarm".into())
        .spawn(sck::prewarm);
}

#[cfg(target_os = "macos")]
pub fn request_screen_capture_access() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

// ---------------------------------------------------------------------------
// window-shot — non-interactive capture of the daemon's own shared window
// ---------------------------------------------------------------------------

pub const WINDOW_SHOT_USAGE: &str =
    "window-shot [--view VIEW] [--out PATH.png] [--scale 1|2]";

/// `window-shot` arguments, tab-separated like `pane-shot`:
/// `--view <view>` (open it first), `--out <path.png>` (default a temp file),
/// `--scale 1|2` (default 2, the retina backing scale). Pure so the parser is
/// unit-testable without a GUI or capture permission.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowShotArgs {
    pub view: Option<String>,
    pub out: Option<String>,
    pub scale: f64,
}

impl Default for WindowShotArgs {
    fn default() -> Self {
        WindowShotArgs {
            view: None,
            out: None,
            scale: 2.0,
        }
    }
}

impl WindowShotArgs {
    pub fn parse(words: &[String]) -> Result<WindowShotArgs, CaptureError> {
        let mut a = WindowShotArgs::default();
        let mut i = 0usize;
        while i < words.len() {
            let w = words[i].as_str();
            match w {
                "--view" | "-v" => a.view = Some(ws_value(words, &mut i, w)?),
                "--out" | "-o" => a.out = Some(ws_value(words, &mut i, w)?),
                "--scale" | "-s" => {
                    let v = ws_value(words, &mut i, w)?;
                    match v.as_str() {
                        "1" => a.scale = 1.0,
                        "2" => a.scale = 2.0,
                        other => {
                            return Err(CaptureError::new(format!(
                                "--scale is 1 or 2, not {other}"
                            )))
                        }
                    }
                }
                "-h" | "--help" => {
                    return Err(CaptureError::new(format!(
                        "usage: kitchen-sink {WINDOW_SHOT_USAGE}"
                    )))
                }
                _ => {
                    return Err(CaptureError::new(format!(
                        "unknown argument {w}\nusage: kitchen-sink {WINDOW_SHOT_USAGE}"
                    )))
                }
            }
            i += 1;
        }
        Ok(a)
    }
}

fn ws_value(words: &[String], i: &mut usize, flag: &str) -> Result<String, CaptureError> {
    *i += 1;
    if *i >= words.len() || words[*i].is_empty() {
        return Err(CaptureError::new(format!("{flag} needs a value")));
    }
    Ok(words[*i].clone())
}

/// Capture one window by its `CGWindowID` via ScreenCaptureKit's
/// desktop-independent window filter, at the requested backing scale, encoded
/// as a PNG. Used by the `window-shot` socket verb — no overlay, no activation.
#[cfg(target_os = "macos")]
/// `window-shot` without Screen Recording permission (the test VM has none): the
/// app draws its own window — the frame view, so the title bar is included —
/// into a bitmap and encodes it as PNG. Main thread only. Views that draw
/// outside AppKit's cache path (a Metal/GL layer) may come out blank.
pub fn window_self_render_png(window_id: u32) -> Result<Vec<u8>, String> {
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSApplication, NSBitmapImageFileType, NSBitmapImageRepPropertyKey};
    use objc2_foundation::NSDictionary;
    let mtm = objc2::MainThreadMarker::new().ok_or("window self-render needs the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    let window = app
        .windowWithWindowNumber(window_id as isize)
        .ok_or_else(|| format!("no window {window_id}"))?;
    let content = window.contentView().ok_or("window has no content view")?;
    let view = unsafe { content.superview() }.unwrap_or(content.clone());
    // Layer-backed views paint backgrounds and some content straight into their
    // CALayers, which `cacheDisplayInRect` skips (it only replays drawRect:), so
    // render the layer tree when there is one.
    if let Some(png) = render_layer_png(&view).or_else(|| render_layer_png(&content)) {
        return Ok(png);
    }
    let rect = view.bounds();
    let rep = view
        .bitmapImageRepForCachingDisplayInRect(rect)
        .ok_or("could not allocate a bitmap for the window")?;
    view.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
    let props: objc2::rc::Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> =
        NSDictionary::new();
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("could not encode PNG")?;
    Ok(png.to_vec())
}

/// `view.layer` rendered into a 2x sRGB bitmap and encoded as PNG; `None`
/// when the view has no layer or the bitmap cannot be made.
#[cfg(target_os = "macos")]
fn render_layer_png(view: &objc2_app_kit::NSView) -> Option<Vec<u8>> {
    use objc2_core_graphics::{
        kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext,
        CGImageAlphaInfo,
    };
    let layer = view.layer()?;
    let b = view.bounds();
    let scale = 2.0;
    let (w, h) = ((b.size.width * scale) as usize, (b.size.height * scale) as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let ctx = unsafe {
        CGBitmapContextCreate(std::ptr::null_mut(), w, h, 8, 0, Some(&*space),
                              CGImageAlphaInfo::PremultipliedLast.0)
    }?;
    // CALayer draws in its own geometry; a flipped root layer needs a flipped context.
    if layer.isGeometryFlipped() {
        CGContext::translate_ctm(Some(&*ctx), 0.0, h as f64);
        CGContext::scale_ctm(Some(&*ctx), scale, -scale);
    } else {
        CGContext::scale_ctm(Some(&*ctx), scale, scale);
    }
    layer.renderInContext(&ctx);
    let img = CGBitmapContextCreateImage(Some(&*ctx))?;
    shot_cg::encode_png(&img)
}

#[cfg(target_os = "macos")]
pub fn window_shot_image(window_id: u32, scale: f64) -> Result<ShotImage, CaptureError> {
    let captured = sck::capture_window(window_id, scale)?;
    let image = captured
        .image
        .as_ref()
        .ok_or_else(|| CaptureError::new("window-shot: no image captured"))?;
    encode_shot_image(image)
        .ok_or_else(|| CaptureError::new("window-shot: could not encode PNG"))
}

// ---------------------------------------------------------------------------
// ScreenToast
// ---------------------------------------------------------------------------

/// One toast clock for the app: shares [`crate::ui::toast::HOLD_SECS`] so the
/// capture overlay's pill and the `show()` pill can't drift apart.
pub const TOAST_DURATION: f64 = crate::ui::toast::HOLD_SECS;

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
        // The model slot is what tests read; the live pill is the AppKit pass
        // (a no-op off the main thread).
        #[cfg(target_os = "macos")]
        crate::ui::toast::show(&text, symbol);
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

use crate::app::logging::now_unix as unix_now;

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

/// `[screenshot] history-dir` default: the Recents folder.
pub const DEFAULT_HISTORY_DIR: &str = "~/.cache/kitchen-sink/screenshots";

/// The folder Hyper+X captures are kept in (`[screenshot] history-dir`),
/// absolute. What the Files sidebar's "Screenshots" pin opens.
pub fn screenshots_dir() -> String {
    let d = ScreenshotConfig::load().history_dir;
    expand_tilde(if d.trim().is_empty() { DEFAULT_HISTORY_DIR } else { d.trim() })
}

/// Longest side (pixels) a picture opened in the editor is decoded at; a
/// 48 MP photo (8064 px) still comes in at native size.
pub const EDITOR_MAX_IMAGE_PX: i64 = 8192;

/// The file-list title for sending `count` pictures to the editor.
pub fn editor_menu_title(m: &MergeConfig, count: usize) -> String {
    if count <= 1 {
        m.editor_menu_title.clone()
    } else {
        m.editor_merge_menu_title.replace("{}", &count.to_string())
    }
}

/// File lists (Find Recent Files, the Files view) ▸ right-click: the
/// "Open in Screenshot Editor" / "Merge N Photos in Screenshot Editor" row,
/// or `None` unless `[screenshot]` is on and **every** selected path is a
/// real picture (judged by its bytes, `image_files::all_images`).
#[cfg(target_os = "macos")]
pub fn editor_menu_entry(
    shot: &Arc<Mutex<ScreenshotController>>,
    paths: &[String],
) -> Option<crate::ui::menu_kit::MenuEntry> {
    if !crate::engines::image_files::all_images(paths) {
        return None;
    }
    let cfg = ScreenshotConfig::load();
    if !cfg.enabled {
        return None;
    }
    let title = editor_menu_title(&cfg.merge, paths.len());
    let (shot, paths) = (shot.clone(), paths.to_vec());
    Some(crate::ui::menu_kit::MenuEntry::item(&title, move || {
        // After the menu has closed, so the overlay takes the keyboard.
        let (shot, paths) = (shot.clone(), paths.clone());
        crate::platform::run_on_main(move || {
            if let Ok(mut c) = shot.lock() {
                c.open_images_in_editor(&paths);
            }
        });
    }))
}

/// Files / Find Recent Files ▸ right-click on pictures ▸ "Copy Text from
/// Picture(s)": the same Copy Text (OCR, QR codes, the card) as Hyper+X, on
/// pictures you already have. `[screenshot] text-menu-title`.
pub fn copy_text_menu_entry(
    shot: &Arc<Mutex<ScreenshotController>>,
    paths: &[String],
) -> Option<crate::ui::menu_kit::MenuEntry> {
    if !crate::engines::image_files::all_images(paths) {
        return None;
    }
    let cfg = ScreenshotConfig::load();
    if !cfg.enabled || cfg.text_menu_title.is_empty() {
        return None;
    }
    let title = if paths.len() == 1 {
        cfg.text_menu_title.clone()
    } else {
        format!("{} ({})", cfg.text_menu_title, paths.len())
    };
    let (shot, paths) = (shot.clone(), paths.to_vec());
    Some(crate::ui::menu_kit::MenuEntry::item(&title, move || {
        let (shot, paths) = (shot.clone(), paths.clone());
        crate::platform::run_on_main(move || {
            if let Ok(mut c) = shot.lock() {
                c.copy_text_from_files(&paths);
            }
        });
    }))
}

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

#[cfg(target_os = "macos")]
use crate::ui::popup::key_input_from_event;

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
    /// Menu bar height above the usable area (chrome sits below it).
    pub top_inset: f64,
}

impl ShotDisplay {
    pub fn new(id: u32, frame: Rect, scale: f64) -> Self {
        ShotDisplay {
            id,
            frame,
            scale,
            key: false,
            wid: 0,
            top_inset: 0.0,
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

impl WheelState {
    /// `ShotWheelView.index(at:center:count:)` against this wheel's hub.
    pub fn hit(&self, p: Point, count: usize) -> Option<i64> {
        ShotWheel::index_at(p, self.center, count).map(|i| i as i64)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveCardState {
    pub display: usize,
    pub field: String,
    /// Selected `[start, end)` char range of `field` (Swift `ShotSaveCard.focus`
    /// selects the file-name stem so typing replaces it); typing, paste and
    /// Delete replace it, ⌘A selects everything.
    pub sel: Option<(usize, usize)>,
}

impl SaveCardState {
    /// Replace the selection (or append) with `text`.
    pub fn insert(&mut self, text: &str) {
        let n = self.field.chars().count();
        let (a, b) = self.sel.take().map(|(a, b)| (a.min(n), b.min(n))).unwrap_or((n, n));
        let head: String = self.field.chars().take(a).collect();
        let tail: String = self.field.chars().skip(b).collect();
        self.field = format!("{head}{text}{tail}");
    }
    /// Delete the selection, or the last char.
    pub fn backspace(&mut self) {
        if self.sel.is_some() {
            self.insert("");
        } else {
            self.field.pop();
        }
    }
    pub fn select_all(&mut self) {
        self.sel = Some((0, self.field.chars().count()));
    }
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
    /// Resizing a selected shape (8 handles for boxes, 2 endpoints for lines/arrows).
    ResizingObject {
        index: usize,
        handle: i64,
        from: ShotObject,
        start: Point,
        moved: bool,
    },
    /// Scaling the text draft by a box handle: the opposite handle stays put.
    ScalingText {
        anchor: Point,
        /// Anchor position as fractions of the box (0..1).
        frac: (f64, f64),
        d0: f64,
        size0: i64,
    },
    /// Dragging the text draft by its box.
    MovingText {
        from: Point,
        start: Point,
    },
}

/// The side panel's hex field buffer. `replace` = the whole text is
/// selected (on focus / Cmd+A), so the next typed character replaces it.
#[derive(Clone, Debug, PartialEq)]
pub struct HexEdit {
    pub text: String,
    pub replace: bool,
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
    /// Dedicated colour for the Highlighter (`ShotTool::Marker`), defaulting
    /// to bright yellow (`cfg.highlighter_color`) independently of pen/shape colour.
    pub marker_color: ShotColor,
    pub counter_offset: i64,
    pub side_panel_open: bool,
    pub grabbing: Option<ShotColor>,
    /// The hex the last colour pick copied (`grab-copies-hex`).
    pub picked_hex: Option<String>,
    /// This grab was started with Shift+G: copy the picked hex.
    pub grab_copies: bool,
    /// The grab was started from the panel's Grab Color button: the panel
    /// hides for the grab and comes back when it ends (pick or cancel).
    grab_from_panel: bool,
    /// The side panel's hex field while it has the keyboard.
    pub hex_edit: Option<HexEdit>,
    /// The colour under a live drag on the panel's wheel (applied on up).
    panel_wheel: Option<ShotColor>,
    pub editing: Option<TextEditState>,
    pub wheel: Option<WheelState>,
    pub shortcuts_shown: bool,
    pub save_card: Option<SaveCardState>,
    pub chosen_save_path: Option<String>,
    pub finished: bool,
    pub outcome: Option<ShotOutcome>,
    pub recent_requested: bool,
    /// V: record the selection as a video once the overlay closes.
    pub record_requested: bool,
    /// Cmd+T: the controller re-runs the capture of this area after the delay.
    pub retake_requested: bool,
    /// The recents grid is open (the pill's third stop). Kept in sync by the
    /// controller's `toggle_recent` / `close_recent`.
    pub recent_open: bool,
    pub chrome_rects: std::cell::Cell<Option<ChromeRects>>,
    chrome_press: Option<ChromePress>,
    pub mouse: Option<(usize, Point)>,
    pub primary_max_y: f64,
    ring_new: bool,
    last_wheel_ms: i64,
    drag: Drag,
    /// A merged stack (Recents merge / Files ▸ Merge N Photos): the reorder
    /// strip's photos and the layout to rebuild the canvas from.
    #[cfg(target_os = "macos")]
    pub merge: Option<MergeEdit>,
    /// A base image was replaced in place (a merge reorder): the overlay
    /// panels must reload their layer contents.
    pub base_dirty: bool,
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
        let marker_color = state
            .marker_color
            .as_deref()
            .and_then(ShotColor::from_hex)
            .unwrap_or(cfg.highlighter_color);
        let mut s = ShotSession {
            doc: ShotDocument::new(cfg.undo_limit.max(1) as usize),
            color,
            marker_color,
            text_mode: args.mode == ShotMode::Text
                // A preset area (`--region`, `--last-region`, a Cmd+T retake)
                // means editing a screenshot, not copying text.
                || (args.mode == ShotMode::Gui
                    && cfg.start_text
                    && args.region.is_none()
                    && !args.last_region),
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
            picked_hex: None,
            grab_copies: false,
            grab_from_panel: false,
            hex_edit: None,
            panel_wheel: None,
            editing: None,
            wheel: None,
            shortcuts_shown: false,
            save_card: None,
            chosen_save_path: None,
            finished: false,
            outcome: None,
            recent_requested: false,
            record_requested: false,
            retake_requested: false,
            recent_open: false,
            chrome_rects: std::cell::Cell::new(None),
            chrome_press: None,
            mouse: None,
            primary_max_y: 0.0,
            ring_new: true,
            last_wheel_ms: 0,
            drag: Drag::None,
            #[cfg(target_os = "macos")]
            merge: None,
            base_dirty: false,
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
        if let Some(v) = self.state.sizes.get(t.raw_value()) {
            return *v;
        }
        let configured = if t == ShotTool::Text {
            self.cfg.draw_font_size
        } else if t.default_size() == 3 {
            self.cfg.draw_thickness
        } else {
            0
        };
        if configured > 0 {
            let (lo, hi) = t.size_range();
            configured.clamp(lo, hi)
        } else {
            t.default_size()
        }
    }
    pub fn active_size(&self) -> Option<i64> {
        self.tool.map(|t| self.size(t))
    }
    pub fn current_color(&self) -> ShotColor {
        if let Some(e) = &self.editing {
            return e.object.color;
        }
        match self.doc.selected {
            Some(i) if i < self.doc.objects.len() => self.doc.objects[i].color,
            _ if self.tool == Some(ShotTool::Marker) => self.marker_color,
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
    /// The pill's current stop: 0 Copy Text, 1 Screenshot, 2 Recent.
    pub fn stop(&self) -> usize {
        if self.recent_open {
            2
        } else if self.text_mode {
            0
        } else {
            1
        }
    }

    /// Move to stop `n` (0 Copy Text, 1 Screenshot, 2 Recent). Leaving or
    /// re-selecting Recent closes the grid; reaching it opens the grid, via
    /// `recent_requested` (the controller toggles it).
    pub fn goto_stop(&mut self, n: usize) {
        let cur = self.stop();
        if n == cur {
            if cur == 2 {
                self.recent_requested = true;
            }
            return;
        }
        if cur == 2 {
            self.recent_requested = true;
        }
        // Copy Text and Screenshot are separate tasks: switching between them
        // starts clean, so a region (and its annotations) drawn in one mode
        // does not carry over into the other (Tab, draw, Tab back left the
        // screenshot rectangle up in Copy Text).
        let switching = (n == 0 && !self.text_mode) || (n == 1 && self.text_mode);
        if switching {
            self.clear_selection();
        }
        match n {
            0 if !self.text_mode => self.toggle_text_mode(),
            1 if self.text_mode => self.toggle_text_mode(),
            2 => self.recent_requested = true,
            _ => {}
        }
    }

    /// `Tab` / `Shift+Tab` through Copy Text → Screenshot → Recent.
    pub fn cycle_stop(&mut self, back: bool) {
        let cur = self.stop();
        self.goto_stop(if back { (cur + 2) % 3 } else { (cur + 1) % 3 });
    }

    /// Drop the region and everything drawn in it (a fresh start, as on open).
    pub fn clear_selection(&mut self) {
        self.editing = None;
        self.close_save_card();
        self.doc.reset();
        self.selection = None;
        self.drag = Drag::None;
    }

    pub fn toggle_text_mode(&mut self) {
        self.commit_text();
        self.close_wheel();
        if let Some(prev) = self.grabbing {
            self.color = prev;
            self.grabbing = None;
        }
        self.grab_from_panel = false;
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
        if let Some(e) = self.editing.as_mut() {
            e.object.color = c;
            self.color = c;
            self.state.color = Some(c.hex());
            self.save_state();
            return;
        }
        if let Some(i) = self.doc.selected {
            if i < self.doc.objects.len() {
                let is_marker = self.doc.objects[i].tool == ShotTool::Marker;
                self.doc.update(i, None, |o| o.color = c);
                if is_marker {
                    self.marker_color = c;
                    self.state.marker_color = Some(c.hex());
                    self.save_state();
                }
            }
        } else if self.tool == Some(ShotTool::Marker) {
            self.marker_color = c;
            self.state.marker_color = Some(c.hex());
            self.save_state();
        } else {
            self.color = c;
            self.state.color = Some(c.hex());
            self.save_state();
        }
    }
    pub fn change_size(&mut self, delta: i64) {
        if let Some(e) = self.editing.as_mut() {
            let (lo, hi) = ShotTool::Text.size_range();
            let v = (e.object.size + delta).clamp(lo, hi);
            e.object.size = v;
            self.state.sizes.insert(ShotTool::Text.raw_value().to_string(), v);
            self.save_state();
            return;
        }
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
        let cur = if let Some(e) = &self.editing {
            Some(e.object.size)
        } else {
            match self.doc.selected {
                Some(i) if i < self.doc.objects.len() => Some(self.doc.objects[i].size),
                _ => self.tool.map(|t| self.size(t)),
            }
        };
        if let Some(cur) = cur {
            self.change_size(v - cur);
        }
    }

    // -- selection --------------------------------------------------------

    pub fn new_object(&self, t: ShotTool, p: Point) -> ShotObject {
        let color = if t == ShotTool::Marker {
            self.marker_color
        } else {
            self.color
        };
        let mut o = ShotObject::new(t, vec![p, p], color, self.size(t));
        o.open_arrow = self.cfg.arrow_style == 1;
        o.reversed = self.cfg.reverse_arrow;
        o.outline = self.cfg.counter_outline;
        o.secure = !self.cfg.insecure_pixelate;
        o.style = self.state.style.clone();
        if t == ShotTool::Counter {
            o.points = vec![p];
            o.number_offset = self.counter_offset;
        }
        if t == ShotTool::Pencil || t == ShotTool::Marker {
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

    /// L: select the area the last capture used (saved for every capture
    /// that was not cancelled). `false` when there is none or its display is
    /// gone.
    pub fn select_last_region(&mut self) -> bool {
        let Some(l) = self.state.last_region.clone() else { return false };
        let Some(i) = self.displays.iter().position(|d| d.id == l.display) else { return false };
        self.select(Some(Rect::new(l.x, l.y, l.w, l.h)), Some(i), true);
        self.selection.is_some()
    }

    /// W: select the window under the pointer (macOS's Cmd+Shift+4 then
    /// Space, CleanShot's window mode) — the frozen image is cropped to it.
    pub fn select_window_under_mouse(&mut self, windows: &[ScreenWindow]) -> bool {
        let Some((d, p)) = self.mouse else { return false };
        let Some(disp) = self.displays.get(d).cloned() else { return false };
        let o = self.global_origin(&disp);
        let at = Point::new(p.x + o.x, p.y + o.y);
        let Some(r) = window_under(windows, at, std::process::id() as i32) else { return false };
        let local = Rect::new(r.x - o.x, r.y - o.y, r.w, r.h);
        // Clip to the display the pointer is on.
        let b = disp.bounds();
        let x0 = local.x.max(b.x);
        let y0 = local.y.max(b.y);
        let x1 = (local.x + local.w).min(b.x + b.w);
        let y1 = (local.y + local.h).min(b.y + b.h);
        if x1 - x0 < 4.0 || y1 - y0 < 4.0 {
            return false;
        }
        self.select(Some(Rect::new(x0, y0, x1 - x0, y1 - y0)), Some(d), true);
        true
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

    /// Edit the live text draft in place. The painted draft is `object.text`,
    /// so keep both in sync (the model's `text` is the editable buffer).
    fn edit_text<F: FnOnce(&mut String)>(&mut self, f: F) {
        if let Some(e) = self.editing.as_mut() {
            f(&mut e.text);
            e.object.text = e.text.clone();
        }
    }

    /// The text-draft box handle under `p` as `(hx, hy)` fractions in
    /// `{0, .5, 1}` of the (outset) box: corners and mid-edges.
    fn text_handle_at(b: Rect, p: Point) -> Option<(f64, f64)> {
        for hy in [0.0, 0.5, 1.0] {
            for hx in [0.0, 0.5, 1.0] {
                if hx == 0.5 && hy == 0.5 {
                    continue;
                }
                let c = Point::new(b.x + hx * b.w, b.y + hy * b.h);
                if (p.x - c.x).abs() <= 7.0 && (p.y - c.y).abs() <= 7.0 {
                    return Some((hx, hy));
                }
            }
        }
        None
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

    /// Cycle the text font family through [`FONT_PRESETS`].
    pub fn cycle_font_family(&mut self, step: i64) {
        let cur = self.panel_text_style().family;
        let n = FONT_PRESETS.len() as i64;
        let idx = FONT_PRESETS
            .iter()
            .position(|(f, _)| f.eq_ignore_ascii_case(&cur))
            .map(|i| i as i64)
            .unwrap_or(-1);
        let next = if idx < 0 {
            0
        } else {
            (idx + step).rem_euclid(n) as usize
        };
        let fam = FONT_PRESETS[next].0.to_string();
        self.set_text_style(|st| st.family = fam.clone());
    }

    // -- side panel / shortcuts / grab / wheel ----------------------------

    pub fn toggle_side_panel(&mut self, open: Option<bool>) {
        if self.panel_display().is_none() {
            return;
        }
        let want = open.unwrap_or(!self.side_panel_open);
        if !want {
            self.hex_edit = None;
            self.panel_wheel = None;
        }
        self.side_panel_open = want;
    }

    /// The display the panel (and its tab) lives on: the one holding the
    /// selection, else the one under the pointer.
    pub fn panel_display(&self) -> Option<usize> {
        self.active.or(self.mouse_display())
    }

    /// The panel layout on display `d` while it is open there.
    pub fn panel_layout(&self, d: usize) -> Option<PanelLayout> {
        if !self.side_panel_open || self.text_mode || self.panel_display() != Some(d) {
            return None;
        }
        let disp = self.displays.get(d)?;
        Some(ShotSidePanel::layout(disp.bounds(), disp.top_inset, self.cfg.side_panel_width))
    }

    /// The Tool Settings tab on display `d` while the panel is closed.
    pub fn tool_tab_rect(&self, d: usize) -> Option<Rect> {
        if !self.cfg.show_side_panel_button
            || self.side_panel_open
            || self.text_mode
            || self.grabbing.is_some()
            || self.panel_display() != Some(d)
        {
            return None;
        }
        Some(ShotSidePanel::tab_rect(self.displays.get(d)?.bounds()))
    }

    /// The merge editor's reorder strip on display `d` (2+ photos, not in
    /// text mode). It sits right of the Tool Settings tab, or of the side
    /// panel while that is open.
    #[cfg(target_os = "macos")]
    pub fn merge_strip(&self, d: usize) -> Option<MergeStripLayout> {
        let m = self.merge.as_ref()?;
        if m.display != d || self.text_mode || self.save_card.is_some() {
            return None;
        }
        let disp = self.displays.get(d)?;
        let b = disp.bounds();
        let x0 = match self.panel_layout(d) {
            Some(l) => l.panel.max_x() + MergeStrip::INSET,
            None => b.x + ShotToolTab::WIDTH + MergeStrip::INSET,
        };
        MergeStrip::layout(b, disp.top_inset, x0, m.images.len())
    }

    /// A press on the reorder strip: picks a row up (or swallows the click
    /// on the strip's padding). `false` when `p` is not on the strip.
    #[cfg(target_os = "macos")]
    fn merge_strip_press(&mut self, d: usize, p: Point) -> bool {
        let Some(l) = self.merge_strip(d) else { return false };
        if !l.strip.contains_point(p) {
            return false;
        }
        if let Some(h) = l.direction.iter().position(|r| r.contains_point(p)) {
            self.set_merge_horizontal(Some(h == 1));
            self.chrome_press = Some(ChromePress::Dead);
            return true;
        }
        match MergeStrip::row_at(&l, p) {
            Some(i) => {
                if let Some(m) = self.merge.as_mut() {
                    m.drag = Some((i, i));
                }
                self.chrome_press = Some(ChromePress::MergeRow);
            }
            None => self.chrome_press = Some(ChromePress::Dead),
        }
        true
    }

    /// Move photo `from` to slot `to` in a merged stack (the strip drag).
    #[cfg(target_os = "macos")]
    pub fn move_merged(&mut self, from: usize, to: usize) -> bool {
        let Some(m) = self.merge.as_ref() else { return false };
        let n = m.images.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        let mut order: Vec<usize> = (0..n).collect();
        move_item(&mut order, from, to);
        let h = m.horizontal;
        self.rebuild_merge(&order, h)
    }

    /// Stack the merged photos the other way (the strip's ↓ / → toggle):
    /// `Some(h)` sets it, `None` flips it. `false` when nothing changed.
    #[cfg(target_os = "macos")]
    pub fn set_merge_horizontal(&mut self, horizontal: Option<bool>) -> bool {
        let Some(m) = self.merge.as_ref() else { return false };
        let want = horizontal.unwrap_or(!m.horizontal);
        if want == m.horizontal {
            return false;
        }
        let order: Vec<usize> = (0..m.images.len()).collect();
        self.rebuild_merge(&order, want)
    }

    /// Rebuild a merged stack with its photos in `order` (`order[slot]` =
    /// the current index of the photo for that slot), stacked `horizontal`:
    /// a new canvas, each number moved to its slot's cell and every
    /// annotation carried with the photo it sits on (scaled with it when
    /// the stack had to change size to fit). The selection becomes the new
    /// stack. Clears undo: the old snapshots belong to the old canvas.
    #[cfg(target_os = "macos")]
    fn rebuild_merge(&mut self, order: &[usize], horizontal: bool) -> bool {
        let mcfg = self.cfg.merge.clone();
        let outline = self.cfg.counter_outline;
        let Some(m) = self.merge.as_ref() else { return false };
        let n = m.images.len();
        if order.len() != n || order.iter().any(|&i| i >= n) {
            return false;
        }
        let Some(disp) = self.displays.get(m.display) else { return false };
        let images: Vec<BaseImage> = order.iter().map(|&i| m.images[i].clone()).collect();
        let sizes: Vec<(f64, f64)> = images
            .iter()
            .map(|i| {
                (
                    objc2_core_graphics::CGImage::width(Some(i)) as f64,
                    objc2_core_graphics::CGImage::height(Some(i)) as f64,
                )
            })
            .collect();
        let gap = mcfg.gap * m.px;
        let layout = merge_layout(&sizes, horizontal, mcfg.gutter * m.px, gap, gap);
        let stage = merge_stage_left(
            layout.width,
            layout.height,
            disp.frame.w,
            disp.frame.h,
            disp.top_inset,
            MergeStrip::reserve(),
            48.0,
            disp.scale,
            mcfg.max_scale,
        );
        let canvas_size = (
            (disp.frame.w * stage.scale).round() as i64,
            (disp.frame.h * stage.scale).round() as i64,
        );
        let Some(canvas) = shot_cg::merge_canvas(
            &images,
            canvas_size.0,
            canvas_size.1,
            &stage,
            &layout,
            mcfg.background,
            mcfg.border_color,
            mcfg.border_width * m.px,
        ) else {
            return false;
        };
        let to_pt = |st: &MergeStage, r: Rect| {
            let k = st.fit / st.scale;
            Rect::new(st.rect.x + r.x * k, st.rect.y + r.y * k, r.w * k, r.h * k)
        };
        let old_rects: Vec<Rect> = m.layout.images.iter().map(|r| to_pt(&m.stage, *r)).collect();
        let new_rects: Vec<Rect> = layout.images.iter().map(|r| to_pt(&stage, *r)).collect();
        let labels = merge_labels(&stage, &layout, &mcfg, m.px, outline);
        // Old stage points per new stage point (1 unless the fit changed).
        let ratio = (stage.fit / stage.scale) / (m.stage.fit / m.stage.scale);
        let display = m.display;
        for o in self.doc.objects.iter_mut() {
            if o.tool == ShotTool::Counter && o.fixed_number && o.number >= 1 && (o.number as usize) <= n {
                // A slot number: it stays "k" and goes to slot k's cell.
                o.points = labels[o.number as usize - 1].points.clone();
                continue;
            }
            let b = o.bbox();
            let c = Point::new(b.mid_x(), b.mid_y());
            let Some(old) = old_rects.iter().position(|r| r.contains_point(c)) else { continue };
            let Some(slot) = order.iter().position(|&i| i == old) else { continue };
            let (a, z) = (old_rects[old], new_rects[slot]);
            o.points = o
                .points
                .iter()
                .map(|p| Point::new(z.x + (p.x - a.x) * ratio, z.y + (p.y - a.y) * ratio))
                .collect();
        }
        let Some(m) = self.merge.as_mut() else { return false };
        m.paths = order.iter().map(|&i| m.paths[i].clone()).collect();
        m.images = images;
        m.layout = layout;
        m.stage = stage.clone();
        m.canvas = canvas_size;
        m.horizontal = horizontal;
        m.drag = None;
        self.doc.clear_history();
        self.doc.selected = None;
        self.set_base_image(display, Some(canvas));
        self.test_select(stage.rect, Some(display));
        self.base_dirty = true;
        true
    }

    /// The size the panel's size control edits: the live text draft's, the
    /// selected object's, else the active tool's, as `(value, lo, hi)`.
    pub fn panel_size(&self) -> Option<(i64, i64, i64)> {
        if let Some(e) = &self.editing {
            let (lo, hi) = ShotTool::Text.size_range();
            return Some((e.object.size, lo, hi));
        }
        let t = match self.doc.selected {
            Some(i) if i < self.doc.objects.len() => {
                let o = &self.doc.objects[i];
                let (lo, hi) = o.tool.size_range();
                return Some((o.size, lo, hi));
            }
            _ => self.tool?,
        };
        let (lo, hi) = t.size_range();
        Some((self.size(t), lo, hi))
    }

    /// The tool the quick-style bar / size badge is currently targeting.
    pub fn active_style_tool(&self) -> Option<ShotTool> {
        if self.editing.is_some() {
            return Some(ShotTool::Text);
        }
        if let Some(i) = self.doc.selected {
            if let Some(o) = self.doc.objects.get(i) {
                return Some(o.tool);
            }
        }
        self.tool.filter(|t| t.is_drawing())
    }

    /// Human-readable size label for the quick-style bar (`16pt` for Text,
    /// `3px` for stroke tools, `12px` for Highlighter).
    pub fn quick_size_label(&self) -> Option<String> {
        let t = self.active_style_tool()?;
        let (v, _, _) = self.panel_size()?;
        let text = match t {
            ShotTool::Text => format!("{}pt", v + 8),
            ShotTool::Marker => format!("{}px", v * 2 + 2),
            ShotTool::Pixelate => format!("{}x", v),
            ShotTool::Counter => format!("#{}", v),
            _ => format!("{}px", v),
        };
        Some(text)
    }

    /// The floating quick-style bar layout on display `d` when a drawing tool,
    /// selected annotation, or active text draft is present.
    pub fn quick_bar_layout(&self, d: usize) -> Option<QuickBarLayout> {
        if self.text_mode
            || self.is_dragging()
            || self.save_card.is_some()
            || self.wheel.is_some()
            || self.grabbing.is_some()
            || self.shortcuts_shown
            || Some(d) != self.active
        {
            return None;
        }
        self.active_style_tool()?;
        let sel = self.selection?;
        let disp = self.displays.get(d)?;
        let ring = ButtonRing::layout(sel, disp.bounds(), self.cfg.buttons.len(), self.cfg.button_size);
        let side_x = self.panel_layout(d).map(|l| l.panel.max_x()).unwrap_or(0.0);
        Some(ShotQuickBar::layout(
            sel,
            disp.bounds(),
            disp.top_inset,
            side_x,
            &ring.frames,
            &self.cfg.user_colors,
            self.text_section_shown(),
        ))
    }

    /// Apply a click inside the floating quick-style bar. Returns `false` if
    /// `p` is outside it. Keeps any live text draft open so users can recolor,
    /// resize, or switch font while typing.
    fn quick_bar_press(&mut self, d: usize, p: Point) -> bool {
        let Some(l) = self.quick_bar_layout(d) else {
            return false;
        };
        let Some(hit) = ShotQuickBar::hit(&l, p) else {
            return false;
        };
        let step = if self.active_style_tool() == Some(ShotTool::Text) { 2 } else { 1 };
        match hit {
            QuickBarHit::Swatch(i) => {
                if let Some((_, c)) = l.swatches.get(i) {
                    self.set_color(*c);
                }
            }
            QuickBarHit::Picker => {
                self.toggle_side_panel(None);
            }
            QuickBarHit::SizeDown => self.change_size(-step),
            QuickBarHit::SizeUp => self.change_size(step),
            QuickBarHit::FontCycle => self.cycle_font_family(1),
            QuickBarHit::Bold => {
                let on = !self.panel_text_style().bold;
                self.set_text_style(|st| st.bold = on);
            }
            QuickBarHit::Italic => {
                let on = !self.panel_text_style().italic;
                self.set_text_style(|st| st.italic = on);
            }
            QuickBarHit::Body => {}
        }
        self.chrome_press = Some(ChromePress::QuickBar(hit));
        true
    }

    /// Test/socket helper: click at `p` on the quick-style bar on the active display.
    pub fn quick_bar_click(&mut self, p: Point) -> bool {
        let Some(d) = self.active else { return false };
        if self.quick_bar_layout(d).is_none() {
            return false;
        }
        self.mouse_down(d, p, 1, false, false);
        self.mouse_up(d, p, false, false);
        true
    }

    pub fn set_grid(&mut self, on: bool) {
        self.state.grid = on;
        self.save_state();
    }

    pub fn set_grid_size(&mut self, v: i64) {
        self.state.grid_size = v.clamp(ShotSidePanel::GRID_MIN, ShotSidePanel::GRID_MAX);
        self.save_state();
    }

    /// The Grab Color button: hide the panel, grab, re-show afterwards.
    pub fn grab_from_panel(&mut self) {
        self.hex_edit = None;
        self.start_grab();
        self.grab_from_panel = true;
        self.side_panel_open = false;
    }

    fn end_grab(&mut self) {
        self.grabbing = None;
        if std::mem::take(&mut self.grab_from_panel) {
            self.side_panel_open = true;
        }
    }

    /// Commit the hex field (valid applies, invalid reverts) and leave it.
    pub fn commit_hex(&mut self) {
        let Some(h) = self.hex_edit.take() else { return };
        let cur = self.current_color();
        let c = ShotSidePanel::hex_commit(&h.text, cur);
        if c != cur {
            self.set_color(c);
        }
    }

    /// A press inside the panel. Returns `false` when `p` is outside it.
    fn panel_press(&mut self, d: usize, p: Point) -> bool {
        let Some(l) = self.panel_layout(d) else { return false };
        let Some(hit) = ShotSidePanel::hit(&l, p) else {
            return false;
        };
        if hit != PanelHit::Hex {
            self.commit_hex();
        }
        match hit {
            PanelHit::SizeDown => self.change_size(-1),
            PanelHit::SizeUp => self.change_size(1),
            PanelHit::Slider => {
                if let Some((_, lo, hi)) = self.panel_size() {
                    self.set_size(ShotSidePanel::slider_value(l.slider, p.x, lo, hi));
                }
            }
            PanelHit::Grab => self.grab_from_panel(),
            PanelHit::Wheel => {
                let c = ShotSidePanel::wheel_color_clamped(l.wheel, p);
                self.preview_panel_color(c);
            }
            PanelHit::Hex => {
                if self.hex_edit.is_none() {
                    self.commit_text();
                    self.hex_edit = Some(HexEdit {
                        text: self.current_color().hex(),
                        replace: true,
                    });
                }
            }
            PanelHit::Grid => self.set_grid(!self.state.grid),
            PanelHit::GridDown => {
                self.set_grid_size(ShotSidePanel::grid_step(self.state.grid_size, -1))
            }
            PanelHit::GridUp => {
                self.set_grid_size(ShotSidePanel::grid_step(self.state.grid_size, 1))
            }
            PanelHit::Style(i) => {
                if self.text_section_shown() {
                    let on = !Self::style_flag(&self.panel_text_style(), i);
                    self.set_text_style(|st| match i {
                        0 => st.bold = on,
                        1 => st.italic = on,
                        2 => st.underline = on,
                        _ => st.strike = on,
                    });
                }
            }
            PanelHit::Align(i) => {
                if self.text_section_shown() {
                    self.set_text_style(|st| st.align = i as i64);
                }
            }
            PanelHit::LayerRow(r) => {
                self.commit_text();
                let n = self.doc.objects.len();
                self.doc.selected = (r < n).then(|| n - 1 - r);
                // A text layer re-opens for editing, with its resize handles.
                if let Some(i) = self.doc.selected {
                    if self.doc.objects[i].tool == ShotTool::Text {
                        let d = self.active.unwrap_or(0);
                        let at = self.doc.objects[i].start();
                        self.begin_text(d, at, Some(i));
                    }
                }
            }
            PanelHit::LayerDelete => {
                if let Some(e) = self.editing.take() {
                    if let Some(i) = e.index {
                        self.doc.remove(i);
                    }
                    self.doc.selected = None;
                } else if let Some(i) = self.doc.selected {
                    self.doc.remove(i);
                }
            }
            PanelHit::LayerUp => self.move_layer(1),
            PanelHit::LayerDown => self.move_layer(-1),
            PanelHit::Swatch | PanelHit::Body => {}
        }
        self.chrome_press = Some(ChromePress::Panel(hit));
        true
    }

    /// The panel's Text section applies: the text tool is active, or a text
    /// object is selected / being edited (`textBox.isHidden` in Swift).
    pub fn text_section_shown(&self) -> bool {
        self.tool == Some(ShotTool::Text)
            || self.editing.is_some()
            || self
                .doc
                .selected
                .and_then(|i| self.doc.objects.get(i))
                .is_some_and(|o| o.tool == ShotTool::Text)
    }

    /// The style the Text section shows: the edited / selected text's, else
    /// the remembered one new text uses.
    pub fn panel_text_style(&self) -> ShotTextStyle {
        if let Some(e) = &self.editing {
            return e.object.style.clone();
        }
        match self.doc.selected.and_then(|i| self.doc.objects.get(i)) {
            Some(o) if o.tool == ShotTool::Text => o.style.clone(),
            _ => self.state.style.clone(),
        }
    }

    pub fn style_flag(st: &ShotTextStyle, i: usize) -> bool {
        match i {
            0 => st.bold,
            1 => st.italic,
            2 => st.underline,
            _ => st.strike,
        }
    }

    /// Layers ▲/▼: move the selected object one step toward the front (+1)
    /// or back (−1), keeping it selected.
    pub fn move_layer(&mut self, d: i64) {
        let Some(i) = self.doc.selected else { return };
        let to = i as i64 + d;
        if to >= 0 && (to as usize) < self.doc.objects.len() {
            self.doc.reorder(i, to as usize);
        }
    }

    /// A drag that started on the panel's slider or wheel.
    fn panel_drag(&mut self, d: usize, p: Point, hit: PanelHit) {
        let Some(l) = self.panel_layout(d) else { return };
        match hit {
            PanelHit::Slider => {
                if let Some((_, lo, hi)) = self.panel_size() {
                    self.set_size(ShotSidePanel::slider_value(l.slider, p.x, lo, hi));
                }
            }
            PanelHit::Wheel => {
                self.preview_panel_color(ShotSidePanel::wheel_color_clamped(l.wheel, p))
            }
            _ => {}
        }
    }

    /// The wheel's live colour: shown at once, persisted on release.
    fn preview_panel_color(&mut self, c: ShotColor) {
        self.panel_wheel = Some(c);
        if let Some(e) = self.editing.as_mut() {
            e.object.color = c;
        } else if self.doc.selected.is_none() {
            if self.tool == Some(ShotTool::Marker) {
                self.marker_color = c;
            } else {
                self.color = c;
            }
        }
    }

    /// Test/socket entry: a full click (down + up) at `p` on the panel display.
    pub fn panel_click(&mut self, p: Point) -> bool {
        let Some(d) = self.panel_display() else { return false };
        if self.panel_layout(d).is_none() && self.tool_tab_rect(d).is_none() {
            return false;
        }
        self.mouse_down(d, p, 1, false, false);
        self.mouse_up(d, p, false, false);
        true
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
        self.grab_copies = false;
    }
    pub fn cancel_grab(&mut self) {
        if let Some(prev) = self.grabbing {
            self.color = prev;
        }
        self.end_grab();
    }
    pub fn close_wheel(&mut self) {
        self.wheel = None;
    }

    /// `ShotSession.rightDown`: commit any text draft, then open the colour
    /// wheel centred at the pointer.
    pub fn right_down(&mut self, d: usize, p: Point) {
        if self.editing.is_some() {
            self.commit_text();
        }
        self.wheel = Some(WheelState {
            display: d,
            center: p,
            hot: None,
        });
    }

    /// `ShotSession.rightDragged`: track the hot swatch. The wheel lives in its
    /// own display's point space, so a drag on another display clears `hot`.
    pub fn right_dragged(&mut self, d: usize, p: Point) {
        let n = self.cfg.user_colors.len();
        let Some(w) = self.wheel.as_mut() else { return };
        w.hot = if d == w.display { w.hit(p, n) } else { None };
    }

    /// `ShotSession.rightUp`: pick the hot swatch and close the wheel.
    pub fn right_up(&mut self, d: usize, p: Point) {
        let Some(w) = self.wheel.as_ref() else { return };
        let n = self.cfg.user_colors.len();
        let hot = if d == w.display { w.hit(p, n) } else { w.hot };
        if let Some(hot) = hot {
            self.pick_wheel(hot);
            self.close_wheel();
        }
    }

    /// `ShotSession.pickWheel`: apply the swatch, or open the side panel for
    /// the picker swatch (`None`).
    pub fn pick_wheel(&mut self, i: i64) {
        let Some(col) = self
            .cfg
            .user_colors
            .get(i.max(0) as usize)
            .copied()
        else {
            return;
        };
        match col {
            Some(c) => self.set_color(c),
            None => self.toggle_side_panel(Some(true)),
        }
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

    fn handle_at(&self, p: Point) -> Option<i64> {
        let s = self.selection?;
        selection_handle_at(s, p, self.cfg.button_size)
    }

    /// The `ShotButton` ring disc under `p` on display `d`, if any (`ShotButton`
    /// subviews take the click before the selection state machine).
    fn ring_press_at(&self, d: usize, p: Point) -> Option<usize> {
        if self.text_mode || Some(d) != self.active {
            return None;
        }
        let sel = self.selection?;
        let disp = self.displays.get(d)?;
        let layout =
            ButtonRing::layout(sel, disp.bounds(), self.cfg.buttons.len(), self.cfg.button_size);
        ring_index_at(&layout.frames, p)
    }

    /// Whether a ring press on disc `i` is still inside its frame on release.
    fn ring_release_at(&self, d: usize, p: Point, i: usize) -> bool {
        if self.text_mode || Some(d) != self.active {
            return false;
        }
        let Some(sel) = self.selection else { return false };
        let Some(disp) = self.displays.get(d) else {
            return false;
        };
        let layout =
            ButtonRing::layout(sel, disp.bounds(), self.cfg.buttons.len(), self.cfg.button_size);
        layout
            .frames
            .get(i)
            .is_some_and(|f| f.contains_point(p))
    }

    // -- key routing ------------------------------------------------------

    pub fn handle_key(&mut self, k: &KeyInput) -> bool {
        let ch = k.chars.map(|c| c.to_ascii_lowercase());
        let code = k.key_code;
        let is_esc = code == KEY_ESC;
        if self.hex_edit.is_some() {
            return self.hex_key(k, ch, code);
        }
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
            // The save card's field is edited here (no AppKit control): the
            // painted `field` is trimmed/pushed, Return confirms.
            if self.save_card.is_some() {
                if code == KEY_DELETE || code == KEY_FORWARD_DELETE {
                    if let Some(card) = self.save_card.as_mut() {
                        card.backspace();
                    }
                    return true;
                }
                // Rule 1: edit shortcuts work in every text input.
                if (k.cmd || k.ctrl) && ch == Some('v') {
                    if let Some(t) = crate::platform::get_text() {
                        let t: String = t.chars().filter(|c| !c.is_control()).collect();
                        if let Some(card) = self.save_card.as_mut() {
                            card.insert(&t);
                        }
                    }
                    return true;
                }
                if k.cmd && ch == Some('a') {
                    if let Some(card) = self.save_card.as_mut() {
                        card.select_all();
                    }
                    return true;
                }
                if (k.cmd || k.ctrl) && matches!(ch, Some('c') | Some('x')) {
                    if let Some(card) = self.save_card.as_mut() {
                        let n = card.field.chars().count();
                        let (a, b) = card.sel.unwrap_or((0, n));
                        let t: String = card.field.chars().skip(a).take(b.saturating_sub(a)).collect();
                        crate::platform::set_text(&t);
                        if ch == Some('x') {
                            card.insert("");
                        }
                    }
                    return true;
                }
                if code == KEY_RETURN || code == KEY_ENTER {
                    let typed = self.save_card.as_ref().unwrap().field.clone();
                    self.confirm_save(&typed);
                    return true;
                }
                if !k.cmd && !k.ctrl && !k.opt {
                    if let Some(c) = k.chars.filter(|c| !c.is_control()) {
                        if let Some(card) = self.save_card.as_mut() {
                            card.insert(c.encode_utf8(&mut [0; 4]));
                        }
                        return true;
                    }
                }
                return false;
            }
            // The text draft (`ShotTextField`): printable characters append,
            // Backspace deletes; a plain Return inserts a line break (the
            // renderer draws multi-line text), Cmd+Return commits above.
            // Rule 1 edit shortcuts + quick font/style shortcuts work live.
            if self.editing.is_some() {
                let edit = k.cmd || k.ctrl;
                if edit && ch == Some('v') {
                    if let Some(t) = crate::platform::get_text() {
                        let t: String = t.chars().filter(|&c| c != '\r').collect();
                        self.edit_text(|s| s.push_str(&t));
                    }
                    return true;
                }
                if edit && matches!(ch, Some('c') | Some('x')) {
                    if let Some(e) = &self.editing {
                        crate::platform::set_text(&e.text);
                    }
                    if ch == Some('x') {
                        self.edit_text(|s| s.clear());
                    }
                    return true;
                }
                if k.cmd && ch == Some('b') {
                    let on = !self.panel_text_style().bold;
                    self.set_text_style(|st| st.bold = on);
                    return true;
                }
                if k.cmd && ch == Some('i') {
                    let on = !self.panel_text_style().italic;
                    self.set_text_style(|st| st.italic = on);
                    return true;
                }
                if k.cmd && ch == Some('u') {
                    let on = !self.panel_text_style().underline;
                    self.set_text_style(|st| st.underline = on);
                    return true;
                }
                if k.cmd && matches!(ch, Some('=') | Some('+')) {
                    self.change_size(2);
                    return true;
                }
                if k.cmd && ch == Some('-') {
                    self.change_size(-2);
                    return true;
                }
                if code == KEY_DELETE || code == KEY_FORWARD_DELETE {
                    self.edit_text(|t| {
                        t.pop();
                    });
                    return true;
                }
                if code == KEY_RETURN || code == KEY_ENTER {
                    self.edit_text(|t| t.push('\n'));
                    return true;
                }
                if !k.cmd && !k.ctrl && !k.opt {
                    if let Some(c) = k.chars.filter(|c| !c.is_control()) {
                        self.edit_text(|t| t.push(c));
                        return true;
                    }
                }
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
            self.cycle_stop(k.shift);
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
        // Cmd+T: close, wait `retake-delay`, capture the same area again
        // (open a menu or hover something meanwhile).
        if k.cmd && !k.shift && ch == Some('t') {
            if self.selection.is_some() {
                self.retake_requested = true;
                self.finish(ShotOutcome::Abort);
            }
            return true;
        }
        // Cmd+P: pin the shot on the desktop (CleanShot / Shottr).
        if k.cmd && !k.shift && ch == Some('p') {
            if self.selection.is_some() {
                self.finish(ShotOutcome::Pin);
            }
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
        if k.cmd && ch == Some('b') && self.text_section_shown() {
            let on = !self.panel_text_style().bold;
            self.set_text_style(|st| st.bold = on);
            return true;
        }
        if k.cmd && ch == Some('i') && self.text_section_shown() {
            let on = !self.panel_text_style().italic;
            self.set_text_style(|st| st.italic = on);
            return true;
        }
        if k.cmd && matches!(ch, Some('=') | Some('+')) {
            self.change_size(if self.active_style_tool() == Some(ShotTool::Text) { 2 } else { 1 });
            return true;
        }
        if k.cmd && ch == Some('-') {
            self.change_size(if self.active_style_tool() == Some(ShotTool::Text) { -2 } else { -1 });
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
                // G picks a colour to draw with; Shift+G also copies its hex
                // (plain G used to overwrite the clipboard on every pick).
                Some('g') => {
                    self.start_grab();
                    self.grab_copies = k.shift;
                    return true;
                }
                Some('o') => {
                    self.toggle_text_mode();
                    return true;
                }
                Some('w') => {
                    let wins = on_screen_windows();
                    self.select_window_under_mouse(&wins);
                    return true;
                }
                Some('l') => {
                    self.select_last_region();
                    return true;
                }
                // V: record the selection as a video (Hyper+X stops it).
                Some('v') => {
                    if self.selection.is_some() {
                        self.record_requested = true;
                        self.finish(ShotOutcome::Abort);
                    }
                    return true;
                }
                Some('[') | Some('-') => {
                    self.change_size(-1);
                    return true;
                }
                Some(']') | Some('=') | Some('+') => {
                    self.change_size(1);
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

    /// Keys while the panel's hex field has focus. Esc leaves the field only
    /// (rule 6); edit shortcuts work with Cmd or Ctrl (rule 1).
    fn hex_key(&mut self, k: &KeyInput, ch: Option<char>, code: u16) -> bool {
        if code == KEY_ESC {
            self.hex_edit = None;
            return true;
        }
        if code == KEY_RETURN || code == KEY_ENTER || code == KEY_TAB {
            self.commit_hex();
            return true;
        }
        if k.cmd && ch == Some('q') {
            self.finish(ShotOutcome::Abort);
            return true;
        }
        let cur_hex = self.current_color().hex();
        let Some(h) = self.hex_edit.as_mut() else { return false };
        let edit = k.cmd || k.ctrl;
        match ch {
            Some('v') if edit => {
                if let Some(t) = crate::platform::clipboard::get_text() {
                    let t: String = t.trim().chars().filter(|c| ShotSidePanel::hex_char_ok(*c)).collect();
                    if h.replace {
                        h.text.clear();
                    }
                    h.text.push_str(&t);
                    h.text.truncate(7);
                    h.replace = false;
                }
                return true;
            }
            Some('c') if edit => {
                crate::platform::clipboard::set_text(&h.text);
                return true;
            }
            Some('x') if edit => {
                crate::platform::clipboard::set_text(&h.text);
                h.text.clear();
                h.replace = false;
                return true;
            }
            Some('a') if edit => {
                h.replace = true;
                return true;
            }
            Some('z') if edit => {
                h.text = cur_hex;
                h.replace = true;
                return true;
            }
            _ => {}
        }
        if code == KEY_DELETE || code == KEY_FORWARD_DELETE {
            if h.replace {
                h.text.clear();
            } else {
                h.text.pop();
            }
            h.replace = false;
            return true;
        }
        if !k.cmd && !k.ctrl && !k.opt {
            if let Some(c) = k.chars.filter(|c| ShotSidePanel::hex_char_ok(*c)) {
                if h.replace {
                    h.text.clear();
                    h.replace = false;
                }
                if h.text.chars().count() < 7 {
                    h.text.push(c);
                }
            }
        }
        true
    }

    /// The Esc chain (Swift `ShotSession.escape`): save card → text → shortcuts
    /// → color wheel → grab → side panel → close.
    pub fn escape(&mut self) {
        if self.save_card.is_some() {
            self.close_save_card();
            return;
        }
        if self.hex_edit.is_some() {
            self.hex_edit = None;
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
        // Nothing transient is open: Esc cancels the capture (an armed tool or
        // a selected object no longer eats extra presses).
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
        let field = abbreviate_tilde(&path);
        let (loc, len) = ShotSaveCard::focus_range(&field);
        // `focus_range` counts UTF-16 units; paths here are BMP in practice, so
        // chars line up.
        self.save_card = Some(SaveCardState {
            display: d,
            field,
            sel: Some((loc, loc + len)),
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

    /// `mouseMoved(_:_:)`: track the pointer (loupe, cursor, mode pill).
    pub fn mouse_moved(&mut self, d: usize, p: Point) {
        self.mouse = Some((d, p));
        // Flameshot's grab previews the colour under the pointer live; Esc
        // (`cancel_grab`) restores the colour saved in `grabbing`.
        #[cfg(target_os = "macos")]
        if self.grabbing.is_some() && self.doc.selected.is_none() {
            if let Some(c) = self.sample_color_at(d, p) {
                self.color = c;
            }
        }
        // `mouseMoved`: a live wheel follows the pointer's nearest swatch.
        if let Some(w) = self.wheel.as_ref() {
            if w.display == d {
                let n = self.cfg.user_colors.len();
                if let Some(w) = self.wheel.as_mut() {
                    w.hot = w.hit(p, n);
                }
            }
        }
    }

    pub fn mouse_down(&mut self, d: usize, p: Point, click_count: i64, shift: bool, cmd: bool) {
        let _ = (shift, cmd);
        self.mouse = Some((d, p));
        // The mode pill / recent button are subviews in Swift: they take the
        // click before the selection state machine sees it.
        if let Some(c) = self.chrome_rects.get() {
            if c.display == d && !self.is_dragging() {
                if c.recent.contains_point(p) {
                    self.chrome_press = Some(ChromePress::Recent);
                    return;
                }
                if c.pill.contains_point(p) {
                    let x = p.x - c.pill.x;
                    // Segments only; the hairline gap before the chip is dead.
                    if x >= 4.0 && x < 4.0 + c.first_w + c.second_w {
                        self.chrome_press = Some(ChromePress::Pill(ShotModePill::right_segment(x, c.first_w)));
                    } else {
                        self.chrome_press = Some(ChromePress::Dead);
                    }
                    return;
                }
            }
        }
        if self.shortcuts_shown {
            self.hide_shortcuts();
            return;
        }
        // The side panel and its Tool Settings tab sit above the selection.
        if !self.is_dragging() {
            if self.tool_tab_rect(d).is_some_and(|r| r.contains_point(p)) {
                self.chrome_press = Some(ChromePress::Tab);
                return;
            }
            if self.panel_press(d, p) {
                return;
            }
            #[cfg(target_os = "macos")]
            if self.merge_strip_press(d, p) {
                return;
            }
        }
        // Any other click leaves the hex field (Flameshot: focus out commits).
        self.commit_hex();
        // `ShotButton` subviews take the click before the session state
        // machine: a press on a ring disc is remembered and resolved on up.
        if let Some(i) = self.ring_press_at(d, p) {
            self.chrome_press = Some(ChromePress::Ring(i));
            return;
        }
        // Floating quick-style bar (swatches, size, font, B/I): takes clicks
        // before text commit so styling a live text draft never closes it.
        if !self.is_dragging() && self.quick_bar_press(d, p) {
            return;
        }
        if self.wheel.is_some() {
            let hot = self
                .wheel
                .as_ref()
                .and_then(|w| w.hit(p, self.cfg.user_colors.len()));
            if let Some(hot) = hot {
                self.pick_wheel(hot);
            }
            self.close_wheel();
            return;
        }
        if self.grabbing.is_some() {
            // `mouseDown` during a grab: sample the frozen capture under the
            // pointer (the loupe shows it), then apply it as the draw colour.
            #[cfg(target_os = "macos")]
            if let Some(c) = self.sample_color_at(d, p) {
                // Undo the live preview first so a selected object, not the
                // draw colour, takes the pick.
                if let Some(prev) = self.grabbing {
                    self.color = prev;
                }
                self.set_color(c);
                // The picked colour's hex goes on the clipboard too (for CSS,
                // a ticket, a design tool).
                if self.cfg.grab_copies_hex && self.grab_copies {
                    let hex = c.hex();
                    #[cfg(not(test))]
                    {
                        crate::platform::set_text(&hex);
                        crate::ui::toast::show(&self.cfg.hex_toast.replace("{}", &hex), "eyedropper");
                    }
                    self.picked_hex = Some(hex);
                }
            }
            self.end_grab();
            return;
        }
        if let Some(e) = &self.editing {
            let b = e.object.bbox();
            let outer = Rect::new(b.x - 4.0, b.y - 4.0, b.w + 8.0, b.h + 8.0);
            if let Some((hx, hy)) = Self::text_handle_at(outer, p) {
                let anchor = Point::new(outer.x + (1.0 - hx) * outer.w, outer.y + (1.0 - hy) * outer.h);
                let hp = Point::new(outer.x + hx * outer.w, outer.y + hy * outer.h);
                self.drag = Drag::ScalingText {
                    anchor,
                    frac: (1.0 - hx, 1.0 - hy),
                    d0: ShotGeom::dist(hp, anchor).max(1.0),
                    size0: e.object.size,
                };
                return;
            }
            if outer.contains_point(p) {
                self.drag = Drag::MovingText { from: e.object.start(), start: p };
                return;
            }
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
        if in_active && self.selection.is_some() {
            if let Some(i) = self.doc.selected {
                if let Some(o) = self.doc.objects.get(i) {
                    if let Some(h) = object_handle_at(o, p) {
                        self.drag = Drag::ResizingObject {
                            index: i,
                            handle: h,
                            from: o.clone(),
                            start: p,
                            moved: false,
                        };
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
                    // Swift: the first line is centred on the click.
                    let lh = (self.new_object(ShotTool::Text, p).font_size() * 1.2).ceil();
                    self.begin_text(d, Point::new(p.x, p.y - lh / 2.0 - crate::engines::screenshot_annotations::ShotText::PADDING), None);
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
        if let Some(ChromePress::Panel(hit)) = self.chrome_press {
            self.panel_drag(d, p, hit);
            return;
        }
        #[cfg(target_os = "macos")]
        if self.chrome_press == Some(ChromePress::MergeRow) {
            if let Some(l) = self.merge_strip(d) {
                let slot = MergeStrip::slot_at(&l, p.y);
                if let Some((from, _)) = self.merge.as_ref().and_then(|m| m.drag) {
                    if let Some(m) = self.merge.as_mut() {
                        m.drag = Some((from, slot));
                    }
                }
            }
            return;
        }
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::None => {}
            Drag::Selecting { start } => {
                let q = if shift { ShotSnap::square(start, p) } else { p };
                self.select(Some(ShotGeom::rect(start, q)), Some(d), true);
                self.drag = Drag::Selecting { start };
            }
            Drag::Resizing {
                handle,
                from,
                start,
            } => {
                let r =
                    ShotSession::resize(from, handle, (p.x - start.x, p.y - start.y), shift, cmd);
                self.select(Some(r), self.active, true);
                self.drag = Drag::Resizing { handle, from, start };
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
                self.drag = Drag::MovingSelection { from, start };
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
                    ShotTool::Marker => {
                        if shift {
                            o.points = vec![o.start(), ShotSnap::angle(o.start(), p)];
                        } else if o
                            .points
                            .last()
                            .map(|l| ShotGeom::dist(*l, p) >= 2.0)
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
            Drag::ScalingText { anchor, frac, d0, size0 } => {
                if let Some(e) = self.editing.as_mut() {
                    let f = ShotGeom::dist(p, anchor) / d0;
                    let (lo, hi) = ShotTool::Text.size_range();
                    e.object.size = (((size0 as f64 + 8.0) * f).round() as i64 - 8).clamp(lo, hi);
                    let nb = e.object.bbox();
                    let pad = 4.0;
                    let nw = nb.w + 2.0 * pad;
                    let nh = nb.h + 2.0 * pad;
                    e.object.points = vec![Point::new(
                        anchor.x - frac.0 * nw + pad,
                        anchor.y - frac.1 * nh + pad,
                    )];
                }
                self.drag = Drag::ScalingText { anchor, frac, d0, size0 };
            }
            Drag::MovingText { from, start } => {
                if let Some(e) = self.editing.as_mut() {
                    e.object.points = vec![Point::new(from.x + p.x - start.x, from.y + p.y - start.y)];
                }
                self.drag = Drag::MovingText { from, start };
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
            Drag::ResizingObject {
                index,
                handle,
                from,
                start,
                ..
            } => {
                let dv = (p.x - start.x, p.y - start.y);
                let next_pts = match from.tool {
                    ShotTool::Selection
                    | ShotTool::Rectangle
                    | ShotTool::Circle
                    | ShotTool::Pixelate
                    | ShotTool::Invert => {
                        let nr = ShotSession::resize(from.rect(), handle, dv, cmd, shift);
                        vec![Point::new(nr.min_x(), nr.min_y()), Point::new(nr.max_x(), nr.max_y())]
                    }
                    ShotTool::Line | ShotTool::Arrow | ShotTool::Marker => {
                        if handle == 0 {
                            let anchor = from.end();
                            let q = if shift { ShotSnap::angle(anchor, p) } else { p };
                            vec![q, anchor]
                        } else {
                            let anchor = from.start();
                            let q = if shift { ShotSnap::angle(anchor, p) } else { p };
                            vec![anchor, q]
                        }
                    }
                    _ => from.points.clone(),
                };
                self.doc.update_live(index, |o| o.points = next_pts);
                self.drag = Drag::ResizingObject {
                    index,
                    handle,
                    from,
                    start,
                    moved: true,
                };
            }
        }
    }

    pub fn mouse_up(&mut self, d: usize, p: Point, shift: bool, cmd: bool) {
        let _ = (shift, cmd);
        if let Some(press) = self.chrome_press.take() {
            // A ring disc fires only if the release is still inside frame `i`
            // (mirrors the pill/recent resolution below).
            if let ChromePress::Ring(i) = press {
                if self.ring_release_at(d, p, i) {
                    if let Some(t) = self.cfg.buttons.get(i).copied() {
                        self.pressed(t);
                    }
                }
                return;
            }
            if press == ChromePress::MergeRow {
                #[cfg(target_os = "macos")]
                if let Some((from, to)) = self.merge.as_mut().and_then(|m| m.drag.take()) {
                    self.move_merged(from, to);
                }
                return;
            }
            if let ChromePress::Panel(_) = press {
                if let Some(c) = self.panel_wheel.take() {
                    self.set_color(c);
                }
                return;
            }
            if let ChromePress::QuickBar(_) = press {
                return;
            }
            if press == ChromePress::Tab {
                if self.tool_tab_rect(d).is_some_and(|r| r.contains_point(p)) {
                    self.toggle_side_panel(Some(true));
                }
                return;
            }
            if let Some(c) = self.chrome_rects.get().filter(|c| c.display == d) {
                match press {
                    // Left segment = Copy Text, right = Screenshot.
                    ChromePress::Pill(right) if c.pill.contains_point(p) => {
                        self.goto_stop(if right { 1 } else { 0 });
                    }
                    ChromePress::Recent if c.recent.contains_point(p) => self.goto_stop(2),
                    _ => {}
                }
            }
            return;
        }
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
            Drag::Resizing { .. }
            | Drag::MovingSelection { .. }
            | Drag::ScalingText { .. }
            | Drag::MovingText { .. } => {}
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
                    ShotTool::Marker => {
                        let b = ShotGeom::bounds(&o.points);
                        if o.points.len() >= 2 && b.w.max(b.h) >= 2.0 {
                            self.doc.add(o);
                        }
                    }
                    _ => {
                        if r.w.max(r.h) >= 2.0 {
                            let resizable = matches!(
                                o.tool,
                                ShotTool::Selection
                                    | ShotTool::Rectangle
                                    | ShotTool::Circle
                                    | ShotTool::Pixelate
                                    | ShotTool::Invert
                                    | ShotTool::Line
                                    | ShotTool::Arrow
                            );
                            let idx = self.doc.add(o);
                            if resizable {
                                self.doc.selected = Some(idx);
                            }
                        }
                    }
                }
            }
            Drag::MovingObject {
                index,
                from,
                moved,
                ..
            }
            | Drag::ResizingObject {
                index,
                from,
                moved,
                ..
            } => {
                if moved && index < self.doc.objects.len() {
                    let now = self.doc.objects[index].clone();
                    self.doc.update_live(index, |o| *o = from);
                    self.doc.update(index, None, |o| *o = now);
                    self.doc.selected = Some(index);
                }
            }
        }
    }

    /// `ShotCanvas.color(at:)` on the active capture: the sampled colour under
    /// the pointer, or `None` when there is no frozen image / it is out of
    /// bounds.
    #[cfg(target_os = "macos")]
    pub fn sample_color_at(&self, d: usize, p: Point) -> Option<ShotColor> {
        let base = self.base_images.get(d).and_then(|b| b.as_deref())?;
        let fw = self.displays.get(d).map(|x| x.frame.w).unwrap_or(0.0);
        let scale = canvas_scale(objc2_core_graphics::CGImage::width(Some(base)) as i64, fw);
        shot_cg::sample_color(base, scale, p)
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
    }
}

// ---------------------------------------------------------------------------
// ShotSnapshot / ShotEvent — the pure view contract (Wave 3, example X7)
// ---------------------------------------------------------------------------

/// Everything the overlay draws, pushed by the model. Reads existing
/// [`ShotSession`] fields only (capture itself arrives in Wave 6).
#[derive(Clone, Debug, PartialEq)]
pub struct ShotSnapshot {
    pub displays: Vec<ShotDisplay>,
    pub active: Option<usize>,
    pub selection: Option<Rect>,
    pub objects: Vec<ShotObject>,
    pub selected: Option<usize>,
    pub drawing: Option<ShotObject>,
    pub editing: Option<TextEditState>,
    pub wheel: Option<WheelState>,
    pub save_card: Option<SaveCardState>,
    pub tool: Option<ShotTool>,
    pub color: ShotColor,
    pub move_mode: bool,
    pub text_mode: bool,
    pub side_panel_open: bool,
    pub grabbing: Option<ShotColor>,
    pub shortcuts_shown: bool,
    /// The recents grid (the pill's third stop) is open.
    pub recent_open: bool,
    /// A recents-grid open was requested and is not yet serviced.
    pub recent_requested: bool,
    /// The pill's current stop (0 Copy Text, 1 Screenshot, 2 Recent).
    pub stop: usize,
    pub finished: bool,
    pub outcome: Option<ShotOutcome>,
}

impl Default for ShotSnapshot {
    fn default() -> Self {
        ShotSnapshot {
            displays: Vec::new(),
            active: None,
            selection: None,
            objects: Vec::new(),
            selected: None,
            drawing: None,
            editing: None,
            wheel: None,
            save_card: None,
            tool: None,
            color: ShotColor::BLACK,
            move_mode: false,
            text_mode: false,
            side_panel_open: false,
            grabbing: None,
            shortcuts_shown: false,
            recent_open: false,
            recent_requested: false,
            stop: 1,
            finished: false,
            outcome: None,
        }
    }
}

/// User actions reported to the model. Each maps to the [`ShotSession`] method
/// the AppKit overlay calls for that action today (the `ShotOverlayView`
/// mouse/scroll passthroughs, the key monitor's `handle_key`, and the button
/// ring / mode pill).
#[derive(Clone, Debug, PartialEq)]
pub enum ShotEvent {
    Moved { display: usize, point: Point },
    Down { display: usize, point: Point, click_count: i64, shift: bool, cmd: bool },
    Dragged { display: usize, point: Point, shift: bool, cmd: bool },
    Up { display: usize, point: Point, shift: bool, cmd: bool },
    RightDown { display: usize, point: Point },
    RightDragged { display: usize, point: Point },
    RightUp { display: usize, point: Point },
    /// Mouse wheel: the active tool size, or contrast opacity with Cmd.
    Scroll { delta_y: f64, precise: bool, cmd: bool },
    /// A keyDown routed into the session (`ShotSession::handle_key`).
    Key(KeyInput),
    /// A button-ring tool tap (`pressed(_:)`).
    Pressed(ShotTool),
    /// The pill moved to a stop / toggled the recents grid.
    GotoStop(usize),
    /// Tab / Shift+Tab through the pill stops.
    CycleStop { back: bool },
    Undo,
    Redo,
    SelectAll,
    Escape,
}

impl ShotSession {
    /// Everything the overlay draws, read from the existing model fields.
    pub fn snapshot(&self) -> ShotSnapshot {
        ShotSnapshot {
            displays: self.displays.clone(),
            active: self.active,
            selection: self.selection,
            objects: self.doc.objects.clone(),
            selected: self.doc.selected,
            drawing: self.drawing(),
            editing: self.editing.clone(),
            wheel: self.wheel.clone(),
            save_card: self.save_card.clone(),
            tool: self.tool,
            color: self.color,
            move_mode: self.move_mode,
            text_mode: self.text_mode,
            side_panel_open: self.side_panel_open,
            grabbing: self.grabbing,
            shortcuts_shown: self.shortcuts_shown,
            recent_open: self.recent_open,
            recent_requested: self.recent_requested,
            stop: self.stop(),
            finished: self.finished,
            outcome: self.outcome,
        }
    }

    /// Apply a user action, calling the same model methods the AppKit half
    /// calls for it (mouse passthrough → [`ShotSession::mouse_*`]/`right_*`,
    /// keys → [`ShotSession::handle_key`], ring/pill → [`ShotSession::pressed`]
    /// / [`ShotSession::goto_stop`]).
    pub fn handle(&mut self, ev: ShotEvent) {
        match ev {
            ShotEvent::Moved { display, point } => self.mouse_moved(display, point),
            ShotEvent::Down { display, point, click_count, shift, cmd } => {
                self.mouse_down(display, point, click_count, shift, cmd)
            }
            ShotEvent::Dragged { display, point, shift, cmd } => {
                self.mouse_dragged(display, point, shift, cmd)
            }
            ShotEvent::Up { display, point, shift, cmd } => {
                self.mouse_up(display, point, shift, cmd)
            }
            ShotEvent::RightDown { display, point } => self.right_down(display, point),
            ShotEvent::RightDragged { display, point } => self.right_dragged(display, point),
            ShotEvent::RightUp { display, point } => self.right_up(display, point),
            ShotEvent::Scroll { delta_y, precise, cmd } => self.scroll(delta_y, precise, cmd),
            ShotEvent::Key(k) => {
                let _ = self.handle_key(&k);
            }
            ShotEvent::Pressed(t) => self.pressed(t),
            ShotEvent::GotoStop(n) => self.goto_stop(n),
            ShotEvent::CycleStop { back } => self.cycle_stop(back),
            ShotEvent::Undo => self.undo(),
            ShotEvent::Redo => self.redo(),
            ShotEvent::SelectAll => self.select_all(),
            ShotEvent::Escape => self.escape(),
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

/// The `ShotButton` ring disc under `p`: the index of the first button frame
/// containing the point (`ButtonRing::layout`'s frames; an unplaced button is
/// `Rect::ZERO` and contains nothing).
pub fn ring_index_at(frames: &[Rect], p: Point) -> Option<usize> {
    frames.iter().position(|f| f.contains_point(p))
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

/// Where the mode pill and recent button were last painted on a display
/// (written by the overlay render, read by the click hit test).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeRects {
    pub display: usize,
    pub pill: Rect,
    /// Width of the first ("Copy Text") segment.
    pub first_w: f64,
    /// Width of the second ("Screenshot") segment.
    pub second_w: f64,
    /// The recents chip inside the pill's right end.
    pub recent: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChromePress {
    /// `true` = the second ("Screenshot") segment.
    Pill(bool),
    Recent,
    /// A `ShotButton` ring disc (its index in `cfg.buttons`).
    Ring(usize),
    /// A press on the pill's padding or divider: swallowed, does nothing.
    Dead,
    /// A side-panel control (sliders and the wheel keep tracking on drag).
    Panel(PanelHit),
    /// The Tool Settings tab on the left edge.
    Tab,
    /// A row of the merge editor's reorder strip (the drag is in `merge`).
    MergeRow,
    /// A control on the floating quick-style bar.
    QuickBar(QuickBarHit),
}

/// Curated macOS font presets cycled by the Quick-Style Bar's font pill.
pub const FONT_PRESETS: [(&str, &str); 5] = [
    ("", "Sans"),
    ("Menlo", "Mono"),
    ("Georgia", "Serif"),
    ("Avenir Next", "Clean"),
    ("Marker Felt", "Hand"),
];

/// Short display label for a font family name (used on the Quick-Style Bar pill).
pub fn font_preset_label(family: &str) -> &str {
    if let Some((_, label)) = FONT_PRESETS.iter().find(|(f, _)| f.eq_ignore_ascii_case(family)) {
        return label;
    }
    family.split_whitespace().next().unwrap_or("Font")
}

/// Resize handle points for a drawn annotation object:
/// - Box shapes (`Selection`, `Rectangle`, `Circle`, `Pixelate`, `Invert`):
///   8 handles matching `selection_handles` (`0=TL, 1=T, 2=TR, 3=R, 4=BR, 5=B, 6=BL, 7=L`).
/// - Endpoint shapes (`Line`, `Arrow`, and 2-point straight `Marker`):
///   2 endpoint handles (`0=start`, `1=end`).
pub fn object_handles(o: &ShotObject) -> Vec<Point> {
    match o.tool {
        ShotTool::Selection
        | ShotTool::Rectangle
        | ShotTool::Circle
        | ShotTool::Pixelate
        | ShotTool::Invert => selection_handles(o.rect()).to_vec(),
        ShotTool::Line | ShotTool::Arrow => vec![o.start(), o.end()],
        ShotTool::Marker if o.points.len() == 2 => vec![o.start(), o.end()],
        _ => Vec::new(),
    }
}

/// Closest resize handle of `o` within a forgiving 10 pt grab radius.
pub fn object_handle_at(o: &ShotObject, p: Point) -> Option<i64> {
    let mut best: Option<(i64, f64)> = None;
    for (i, h) in object_handles(o).into_iter().enumerate() {
        let d = ShotGeom::dist(p, h);
        if d <= 10.0 && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((i as i64, d));
        }
    }
    best.map(|(i, _)| i)
}

/// What a click on the floating quick-style bar landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickBarHit {
    Swatch(usize),
    Picker,
    SizeDown,
    SizeUp,
    FontCycle,
    Bold,
    Italic,
    Body,
}

/// Pure geometry of the floating quick-style bar in overlay points.
#[derive(Clone, Debug, PartialEq)]
pub struct QuickBarLayout {
    pub bar: Rect,
    pub swatches: Vec<(Rect, ShotColor)>,
    pub picker: Option<Rect>,
    pub size_down: Rect,
    pub size_value: Rect,
    pub size_up: Rect,
    pub font: Option<Rect>,
    pub bold: Option<Rect>,
    pub italic: Option<Rect>,
}

pub struct ShotQuickBar;

impl ShotQuickBar {
    pub const HEIGHT: f64 = 34.0;
    const PAD_X: f64 = 10.0;
    const SWATCH: f64 = 18.0;
    const SWATCH_GAP: f64 = 6.0;
    const DIV_GAP: f64 = 12.0;

    pub fn layout(
        sel: Rect,
        bounds: Rect,
        top_inset: f64,
        side_panel_x: f64,
        ring_frames: &[Rect],
        colors: &[Option<ShotColor>],
        show_text: bool,
    ) -> QuickBarLayout {
        let concrete: Vec<ShotColor> = colors.iter().filter_map(|c| *c).collect();
        let has_picker = colors.iter().any(|c| c.is_none());
        let h = Self::HEIGHT;

        let mut w = Self::PAD_X * 2.0;
        if !concrete.is_empty() {
            w += concrete.len() as f64 * Self::SWATCH
                + (concrete.len().saturating_sub(1)) as f64 * Self::SWATCH_GAP;
        }
        if has_picker {
            if !concrete.is_empty() {
                w += Self::SWATCH_GAP;
            }
            w += 20.0;
        }
        if !concrete.is_empty() || has_picker {
            w += Self::DIV_GAP;
        }
        // Size stepper: [-] [40pt label] [+]
        w += 22.0 + 42.0 + 22.0;
        if show_text {
            // Divider + Font pill + B + I
            w += Self::DIV_GAP + 58.0 + 4.0 + 24.0 + 4.0 + 24.0;
        }

        let min_x = (side_panel_x + 8.0).max(bounds.x + 8.0);
        let max_x = (bounds.max_x() - w - 8.0).max(min_x);
        let bx = (sel.mid_x() - w / 2.0).clamp(min_x, max_x).round();

        let min_y = bounds.y + top_inset.max(24.0) + 16.0 + ShotModePill::HEIGHT + 10.0;
        let max_y = (bounds.max_y() - h - 8.0).max(min_y);
        let ring_top = ring_frames
            .iter()
            .map(|r| r.min_y())
            .fold(sel.min_y(), f64::min);
        let ring_bot = ring_frames
            .iter()
            .map(|r| r.max_y())
            .fold(sel.max_y(), f64::max);

        let above = ring_top - h - 10.0;
        let below = ring_bot + 10.0;
        let by = if above >= min_y {
            above
        } else if below <= max_y {
            below
        } else {
            (sel.min_y() + 10.0).clamp(min_y, max_y)
        }
        .round();

        let bar = Rect::new(bx, by, w, h);
        let mut x = bx + Self::PAD_X;
        let cy = by + h / 2.0;

        let mut swatches = Vec::with_capacity(concrete.len());
        for (idx, c) in concrete.into_iter().enumerate() {
            if idx > 0 {
                x += Self::SWATCH_GAP;
            }
            swatches.push((
                Rect::new(x, cy - Self::SWATCH / 2.0, Self::SWATCH, Self::SWATCH),
                c,
            ));
            x += Self::SWATCH;
        }
        let picker = if has_picker {
            if !swatches.is_empty() {
                x += Self::SWATCH_GAP;
            }
            let r = Rect::new(x, cy - 10.0, 20.0, 20.0);
            x += 20.0;
            Some(r)
        } else {
            None
        };
        if !swatches.is_empty() || picker.is_some() {
            x += Self::DIV_GAP;
        }
        let size_down = Rect::new(x, cy - 11.0, 22.0, 22.0);
        x += 22.0;
        let size_value = Rect::new(x, cy - 11.0, 42.0, 22.0);
        x += 42.0;
        let size_up = Rect::new(x, cy - 11.0, 22.0, 22.0);
        x += 22.0;

        let (font, bold, italic) = if show_text {
            x += Self::DIV_GAP;
            let f = Rect::new(x, cy - 11.0, 58.0, 22.0);
            x += 58.0 + 4.0;
            let b = Rect::new(x, cy - 11.0, 24.0, 22.0);
            x += 24.0 + 4.0;
            let it = Rect::new(x, cy - 11.0, 24.0, 22.0);
            (Some(f), Some(b), Some(it))
        } else {
            (None, None, None)
        };

        QuickBarLayout {
            bar,
            swatches,
            picker,
            size_down,
            size_value,
            size_up,
            font,
            bold,
            italic,
        }
    }

    pub fn hit(l: &QuickBarLayout, p: Point) -> Option<QuickBarHit> {
        if !l.bar.contains_point(p) {
            return None;
        }
        if let Some(i) = l.swatches.iter().position(|(r, _)| r.inset(-2.0, -2.0).contains_point(p)) {
            return Some(QuickBarHit::Swatch(i));
        }
        if l.picker.is_some_and(|r| r.inset(-2.0, -2.0).contains_point(p)) {
            return Some(QuickBarHit::Picker);
        }
        if l.size_down.contains_point(p) {
            return Some(QuickBarHit::SizeDown);
        }
        if l.size_up.contains_point(p) {
            return Some(QuickBarHit::SizeUp);
        }
        if l.font.is_some_and(|r| r.contains_point(p)) {
            return Some(QuickBarHit::FontCycle);
        }
        if l.bold.is_some_and(|r| r.contains_point(p)) {
            return Some(QuickBarHit::Bold);
        }
        if l.italic.is_some_and(|r| r.contains_point(p)) {
            return Some(QuickBarHit::Italic);
        }
        Some(QuickBarHit::Body)
    }
}

/// `ShotRecentButton` — the recents button beside the mode pill.
pub struct ShotRecentButton;

impl ShotRecentButton {

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

/// What a press inside the side panel landed on (Flameshot's
/// `SidePanelWidget` controls, in top-to-bottom order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelHit {
    SizeDown,
    SizeUp,
    Slider,
    Swatch,
    Grab,
    Wheel,
    Hex,
    Grid,
    GridDown,
    GridUp,
    /// Text: bold / italic / underline / strike (0…3).
    Style(usize),
    /// Text: align left / centre / right (0…2).
    Align(usize),
    /// Layers: list row (0 = the topmost object).
    LayerRow(usize),
    LayerDelete,
    LayerUp,
    LayerDown,
    /// Panel background: swallowed so it never reaches the selection.
    Body,
}

/// The side panel's control rects in overlay (display) points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelLayout {
    pub panel: Rect,
    pub size_label: Rect,
    pub size_down: Rect,
    pub size_value: Rect,
    pub size_up: Rect,
    pub slider: Rect,
    pub color_label: Rect,
    pub swatch: Rect,
    pub grab: Rect,
    pub wheel: Rect,
    pub hex: Rect,
    pub grid_box: Rect,
    pub grid_down: Rect,
    pub grid_value: Rect,
    pub grid_up: Rect,
    /// Text section (shown for the text tool / a selected or edited text).
    pub text_label: Rect,
    pub styles: [Rect; 4],
    pub aligns: [Rect; 3],
    /// Layers section: the list (rows of `LAYER_ROW` points) + its buttons.
    pub layers_label: Rect,
    pub layers: Rect,
    pub layer_delete: Rect,
    pub layer_up: Rect,
    pub layer_down: Rect,
}

impl PanelLayout {
    /// The rects as `{name: [x, y, w, h]}` (socket state, for `panel-click`).
    pub fn to_json(&self) -> Value {
        let r = |r: Rect| json!([r.x, r.y, r.w, r.h]);
        json!({
            "panel": r(self.panel), "sizeDown": r(self.size_down), "sizeUp": r(self.size_up),
            "slider": r(self.slider), "swatch": r(self.swatch), "grab": r(self.grab),
            "wheel": r(self.wheel), "hex": r(self.hex), "grid": r(self.grid_box),
            "gridDown": r(self.grid_down), "gridUp": r(self.grid_up),
            "styles": self.styles.iter().map(|x| r(*x)).collect::<Vec<_>>(),
            "aligns": self.aligns.iter().map(|x| r(*x)).collect::<Vec<_>>(),
            "layers": r(self.layers), "layerDelete": r(self.layer_delete),
            "layerUp": r(self.layer_up), "layerDown": r(self.layer_down),
        })
    }
}

/// A Layers row title (`tableView(_:viewFor:row:)`): "3. Arrow",
/// "5. Text: hello", "2. Counter 1".
pub fn layer_title(i: usize, o: &ShotObject) -> String {
    let title = match o.tool {
        ShotTool::Text => format!("Text: {}", o.text.replace('\n', " ")),
        ShotTool::Counter => format!("Counter {}", o.number),
        t => t.display_name().to_string(),
    };
    let title: String = if title.chars().count() > 26 {
        title.chars().take(25).chain(['\u{2026}']).collect()
    } else {
        title
    };
    format!("{}. {title}", i + 1)
}

/// `ShotSidePanel` — the tool-settings drawer (Space / the Tool Settings tab).
/// Pure layout + value mapping; `overlay::draw_side_panel` paints it.
pub struct ShotSidePanel;

impl ShotSidePanel {
    pub const WIDTH: f64 = 250.0;
    pub const PAD: f64 = 16.0;
    pub const GRID_MIN: i64 = 5;
    pub const GRID_MAX: i64 = 50;
    pub const GRID_STEP: i64 = 5;
    /// The tool-tab height on the left edge.
    pub const TAB_HEIGHT: f64 = 130.0;
    /// One Layers list row.
    pub const LAYER_ROW: f64 = 22.0;

    /// `build`'s layer list height: `max(80, bounds.height - y - 50)`.
    pub fn list_height(bounds_height: f64, y: f64) -> f64 {
        80.0f64.max(bounds_height - y - 50.0)
    }

    /// The control layout for a full-height panel `width` wide on the left
    /// edge of `bounds`, starting below `top` (the display's menu-bar inset).
    pub fn layout(bounds: Rect, top: f64, width: f64) -> PanelLayout {
        let p = Self::PAD;
        let iw = width - 2.0 * p;
        let x0 = bounds.x;
        let mut y = bounds.y + top + 20.0;
        let at = |x: f64, y: f64, w: f64, h: f64| Rect::new(x0 + x, y, w, h);
        let size_label = at(p, y, iw, 18.0);
        y += 24.0;
        let size_down = at(p, y, 28.0, 26.0);
        let size_value = at(p + 32.0, y, iw - 64.0, 26.0);
        let size_up = at(width - p - 28.0, y, 28.0, 26.0);
        y += 36.0;
        let slider = at(p, y, iw, 20.0);
        y += 36.0;
        let color_label = at(p, y, iw - 44.0, 24.0);
        let swatch = at(width - p - 36.0, y, 36.0, 24.0);
        y += 36.0;
        let grab = at(p, y, iw, 28.0);
        y += 42.0;
        let side = iw.min(200.0);
        let wheel = at((width - side) / 2.0, y, side, side);
        y += side + 14.0;
        let hex = at(p + (iw - 120.0) / 2.0, y, 120.0, 26.0);
        y += 44.0;
        let grid_box = at(p, y, iw, 20.0);
        y += 28.0;
        let grid_down = at(p, y, 28.0, 26.0);
        let grid_value = at(p + 32.0, y, iw - 64.0, 26.0);
        let grid_up = at(width - p - 28.0, y, 28.0, 26.0);
        y += 40.0;
        // Text: B I U S, then the three alignments (`textBox` in Swift).
        let text_label = at(p, y, iw, 18.0);
        y += 22.0;
        let bw = 30.0;
        let styles = [0.0, 1.0, 2.0, 3.0].map(|i| at(p + i * (bw + 4.0), y, bw, 26.0));
        let ax = p + 4.0 * (bw + 4.0) + 8.0;
        let aw = ((width - p - ax) / 3.0).floor();
        let aligns = [0.0, 1.0, 2.0].map(|i| at(ax + i * aw, y, aw - 2.0, 26.0));
        y += 40.0;
        // Layers: the list fills the rest, buttons at the foot.
        let layers_label = at(p, y, iw, 18.0);
        y += 22.0;
        let bottom = bounds.y + bounds.h - 16.0;
        let list_h = (bottom - 34.0 - y).max(Self::LAYER_ROW * 3.0);
        let layers = at(p, y, iw, list_h);
        y += list_h + 8.0;
        let layer_delete = at(p, y, 40.0, 26.0);
        let layer_up = at(p + 44.0, y, 40.0, 26.0);
        let layer_down = at(p + 88.0, y, 40.0, 26.0);
        PanelLayout {
            panel: Rect::new(x0, bounds.y, width, bounds.h),
            size_label,
            size_down,
            size_value,
            size_up,
            slider,
            color_label,
            swatch,
            grab,
            wheel,
            hex,
            grid_box,
            grid_down,
            grid_value,
            grid_up,
            text_label,
            styles,
            aligns,
            layers_label,
            layers,
            layer_delete,
            layer_up,
            layer_down,
        }
    }

    /// The control under `p`; `None` outside the panel, `Body` on padding.
    pub fn hit(l: &PanelLayout, p: Point) -> Option<PanelHit> {
        if !l.panel.contains_point(p) {
            return None;
        }
        let controls = [
            (l.size_down, PanelHit::SizeDown),
            (l.size_up, PanelHit::SizeUp),
            (l.slider, PanelHit::Slider),
            (l.swatch, PanelHit::Swatch),
            (l.grab, PanelHit::Grab),
            (l.hex, PanelHit::Hex),
            (l.grid_box, PanelHit::Grid),
            (l.grid_down, PanelHit::GridDown),
            (l.grid_up, PanelHit::GridUp),
            (l.layer_delete, PanelHit::LayerDelete),
            (l.layer_up, PanelHit::LayerUp),
            (l.layer_down, PanelHit::LayerDown),
        ];
        if let Some((_, h)) = controls.iter().find(|(r, _)| r.contains_point(p)) {
            return Some(*h);
        }
        if let Some(i) = l.styles.iter().position(|r| r.contains_point(p)) {
            return Some(PanelHit::Style(i));
        }
        if let Some(i) = l.aligns.iter().position(|r| r.contains_point(p)) {
            return Some(PanelHit::Align(i));
        }
        if l.layers.contains_point(p) {
            return Some(PanelHit::LayerRow(((p.y - l.layers.y) / Self::LAYER_ROW).floor().max(0.0) as usize));
        }
        if Self::wheel_color(l.wheel, p).is_some() {
            return Some(PanelHit::Wheel);
        }
        Some(PanelHit::Body)
    }

    /// The slider value at x (`lo…hi`, clamped to the track).
    pub fn slider_value(track: Rect, x: f64, lo: i64, hi: i64) -> i64 {
        if track.w <= 0.0 || hi <= lo {
            return lo;
        }
        let t = ((x - track.x) / track.w).clamp(0.0, 1.0);
        lo + round_half_away(t * (hi - lo) as f64) as i64
    }

    /// The thumb centre x for `v` on the track.
    pub fn slider_x(track: Rect, v: i64, lo: i64, hi: i64) -> f64 {
        if hi <= lo {
            return track.x;
        }
        let t = ((v - lo) as f64 / (hi - lo) as f64).clamp(0.0, 1.0);
        track.x + t * track.w
    }

    /// The hue/saturation wheel colour at `p` (value = 1); `None` beyond the
    /// rim (a 6pt grace ring clamps to full saturation).
    pub fn wheel_color(wheel: Rect, p: Point) -> Option<ShotColor> {
        let r = wheel.w.min(wheel.h) / 2.0;
        if r <= 0.0 {
            return None;
        }
        let (dx, dy) = (p.x - wheel.mid_x(), p.y - wheel.mid_y());
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > r + 6.0 {
            return None;
        }
        Some(Self::wheel_at(dx, dy, r))
    }

    /// `wheel_color` without the rim cut-off (drags outside keep tracking).
    pub fn wheel_color_clamped(wheel: Rect, p: Point) -> ShotColor {
        let r = (wheel.w.min(wheel.h) / 2.0).max(1.0);
        Self::wheel_at(p.x - wheel.mid_x(), p.y - wheel.mid_y(), r)
    }

    fn wheel_at(dx: f64, dy: f64, r: f64) -> ShotColor {
        let dist = (dx * dx + dy * dy).sqrt();
        let mut h = dy.atan2(dx) / (2.0 * std::f64::consts::PI);
        if h < 0.0 {
            h += 1.0;
        }
        ShotColor::from_hsv(h, (dist / r).min(1.0), 1.0)
    }

    /// Where `c` sits on the wheel (its hue angle and saturation radius).
    pub fn wheel_point(wheel: Rect, c: ShotColor) -> Point {
        let (h, s, _) = c.hsv();
        let r = wheel.w.min(wheel.h) / 2.0 * s;
        let a = h * 2.0 * std::f64::consts::PI;
        Point::new(wheel.mid_x() + a.cos() * r, wheel.mid_y() + a.sin() * r)
    }

    /// Flameshot's hex field: valid input applies, anything else reverts.
    pub fn hex_commit(typed: &str, current: ShotColor) -> ShotColor {
        let t = typed.trim();
        if t.len() == 6 || t.len() == 7 {
            ShotColor::from_hex(t).unwrap_or(current)
        } else {
            current
        }
    }

    /// Characters the hex field accepts.
    pub fn hex_char_ok(c: char) -> bool {
        c == '#' || c.is_ascii_hexdigit()
    }

    /// Grid size `cur` moved `steps` spin-box steps (5…50, step 5).
    pub fn grid_step(cur: i64, steps: i64) -> i64 {
        let snapped = (cur / Self::GRID_STEP) * Self::GRID_STEP;
        (snapped + steps * Self::GRID_STEP).clamp(Self::GRID_MIN, Self::GRID_MAX)
    }

    /// The vertical Tool Settings tab on the left edge of `bounds`.
    pub fn tab_rect(bounds: Rect) -> Rect {
        Rect::new(
            bounds.x,
            bounds.y + ShotToolTab::origin_y(bounds.h, Self::TAB_HEIGHT),
            ShotToolTab::WIDTH,
            Self::TAB_HEIGHT,
        )
    }
}

use crate::app::pathfmt::expand_tilde;

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
    use super::{integral, output_size, px_rect, CapturedImage, ScreenRef, ShotImage};

    /// Toll-free reinterpret one opaque CF reference as another without naming
    /// the `objc2-core-foundation` type (mirrors `AnsiRender.swift`'s helper).
    fn ffi_cast<T, U>(r: &T) -> &U {
        unsafe { &*(r as *const T as *const U) }
    }

    fn cg_rect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
    }

    /// Swift `ScreenshotController.stitch` (~778–793): union every display's
    /// frame × the max backing scale into one bitmap and draw each capture at
    /// `(frame.min - union.min) * scale`. The context is bottom-left (default),
    /// so the raw `draw_image` shows each capture upright (no CTM flip).
    pub fn stitch(screens: &[ScreenRef], images: &[CapturedImage]) -> Option<ShotImage> {
        let mut union: Option<Rect> = None;
        for s in screens {
            union = Some(match union {
                Some(u) => u.union(&s.frame),
                None => s.frame,
            });
        }
        let union = union?;
        let scale = screens.iter().map(|s| s.scale).fold(0.0_f64, f64::max);
        let scale = if scale > 0.0 { scale } else { 2.0 };
        let w = (union.w * scale).round() as i64;
        let h = (union.h * scale).round() as i64;
        if w <= 0 || h <= 0 {
            return None;
        }
        let ctx = bitmap_context(w, h)?;
        CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
        for s in screens {
            let Some(img) = images
                .iter()
                .find(|i| i.display == s.id)
                .and_then(|i| i.image.as_ref())
            else {
                continue;
            };
            let r = Rect::new(
                (s.frame.x - union.x) * scale,
                (s.frame.y - union.y) * scale,
                s.frame.w * scale,
                s.frame.h * scale,
            );
            CGContext::draw_image(Some(&ctx), cg_rect(r), Some(img));
        }
        let out = CGBitmapContextCreateImage(Some(&ctx))?;
        let png = encode_png(&out)?;
        Some(ShotImage::with_png(w, h, png))
    }

    fn alloc_space() -> Option<Retained<CGColorSpace>> {
        let name = unsafe { kCGColorSpaceSRGB };
        CGColorSpace::with_name(Some(name)).map(|s| s.into())
    }

    pub(super) fn bitmap_context(w: i64, h: i64) -> Option<Retained<CGContext>> {
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
    pub fn draw_image(ctx: &CGContext, img: &CGImage, r: Rect, smooth: bool) {
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

    /// `ShotCanvas.color(at:)`: the frozen capture's colour at a point-space
    /// location, scaled to pixels like `draw_loupe` does.
    pub fn sample_color(base: &CGImage, scale: f64, p: Point) -> Option<ShotColor> {
        let px = rasterize(base)?;
        let x = (p.x * scale) as i64;
        let y = (p.y * scale) as i64;
        if x < 0 || y < 0 || x >= px.width || y >= px.height {
            return None;
        }
        Some(px.color(x, y))
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

    /// Recents ▸ merge: the editor canvas for one display (`canvas_w` ×
    /// `canvas_h` px) with the stacked screenshots placed per `stage` /
    /// `layout`, each outlined by a `border_px` line. The numbers are not
    /// drawn here: they are Counter annotations, so the editor can delete them.
    #[allow(clippy::too_many_arguments)]
    pub fn merge_canvas(
        images: &[Retained<CGImage>],
        canvas_w: i64,
        canvas_h: i64,
        stage: &super::MergeStage,
        layout: &super::MergeLayout,
        background: ShotColor,
        border: ShotColor,
        border_px: f64,
    ) -> Option<Retained<CGImage>> {
        let ctx_owner = bitmap_context(canvas_w, canvas_h)?;
        let ctx: &CGContext = &ctx_owner;
        // Top-left, y-down canvas pixels (like `render_shot`).
        CGContext::translate_ctm(Some(ctx), 0.0, canvas_h as f64);
        CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
        set_fill(ctx, background);
        CGContext::fill_rect(Some(ctx), cg_rect(Rect::new(0.0, 0.0, canvas_w as f64, canvas_h as f64)));
        // Into stack pixels.
        CGContext::translate_ctm(Some(ctx), stage.rect.x * stage.scale, stage.rect.y * stage.scale);
        CGContext::scale_ctm(Some(ctx), stage.fit, stage.fit);
        let smooth = stage.fit < 1.0;
        for (img, r) in images.iter().zip(&layout.images) {
            draw_image(ctx, img, *r, smooth);
            // A line just outside each screenshot, so 1 | 2 | 3 read apart.
            if border_px > 0.0 {
                set_stroke(ctx, border);
                CGContext::set_line_width(Some(ctx), border_px);
                let o = border_px / 2.0;
                CGContext::stroke_rect(Some(ctx), cg_rect(Rect::new(r.x - o, r.y - o, r.w + border_px, r.h + border_px)));
            }
        }
        CGBitmapContextCreateImage(Some(ctx)).map(|i| i.into())
    }

    /// Decode a picture file (PNG / JPEG / HEIC / TIFF / GIF / BMP / WebP)
    /// for the editor, upright: ImageIO applies the EXIF orientation, so an
    /// iPhone photo shot in portrait is not drawn on its side. `None` when
    /// the bytes are not a picture (`image_files::sniff_image`) or ImageIO
    /// cannot decode them. Huge images come back at most `max_px` on the
    /// long side (the editor canvas could not hold more anyway).
    pub fn decode_image_file(path: &str, max_px: i64) -> Option<Retained<CGImage>> {
        use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFNumber, CFString, CFType};
        use objc2_image_io::{
            kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceShouldCacheImmediately, kCGImageSourceThumbnailMaxPixelSize, CGImageSource,
        };
        let bytes = std::fs::read(path).ok()?;
        crate::engines::image_files::sniff_image(&bytes)?;
        let data = CFData::from_bytes(&bytes);
        // SAFETY: ImageIO calls with valid CF objects; the source owns its copy.
        unsafe {
            let src = CGImageSource::with_data(&data, None)?;
            if src.count() == 0 {
                return None;
            }
            let index = src.primary_image_index();
            let max = CFNumber::new_i64(max_px.max(1));
            let yes = CFBoolean::new(true);
            let keys: [&CFString; 4] = [
                kCGImageSourceCreateThumbnailFromImageAlways,
                kCGImageSourceCreateThumbnailWithTransform,
                kCGImageSourceShouldCacheImmediately,
                kCGImageSourceThumbnailMaxPixelSize,
            ];
            let values: [&CFType; 4] = [yes.as_ref(), yes.as_ref(), yes.as_ref(), max.as_ref()];
            let opts = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
            src.thumbnail_at_index(index, Some(opts.as_opaque())).map(|i| i.into())
        }
    }

    /// `(string as NSString).size(withAttributes:).width` for the overlay chrome
    /// (help-card columns, button badges), via the shared CoreText font path.
    pub fn measure_text_width(text: &str, point_size: f64, bold: bool) -> f64 {
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

    pub fn draw_text(ctx: &CGContext, o: &ShotObject) {
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
                // Translucent normal-blend wash so the highlighter stays vivid
                // and legible on BOTH light documents and dark terminals/IDEs.
                CGContext::set_blend_mode(Some(ctx), CGBlendMode::Normal);
                let c = o.color.with_alpha(0.38);
                set_stroke(ctx, c);
                set_fill(ctx, c);
                CGContext::set_line_cap(
                    Some(ctx),
                    if o.points.len() <= 2 {
                        CGLineCap::Square
                    } else {
                        CGLineCap::Round
                    },
                );
                CGContext::set_line_join(Some(ctx), CGLineJoin::Round);
                stroke_pts(ctx, &o.points, o.stroke_width());
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

    /// The pixel buffer secure pixelation reads, for the live overlay (the
    /// caller caches it per frozen image; rasterizing is a full-screen copy).
    pub fn pixels_for(base: &CGImage) -> Option<ShotPixels> {
        rasterize(base)
    }

    /// `ShotRenderer.draw(_:canvas:ctx:)` — the objects over the frozen
    /// capture in the context's top-left point space (the live overlay and
    /// the final render share this pass, so what you see is what you get).
    pub fn draw_objects(
        ctx: &CGContext,
        base: &CGImage,
        scale: f64,
        pixels: &Option<ShotPixels>,
        objects: &[&ShotObject],
    ) {
        for o in objects {
            draw_object(ctx, base, scale, pixels, o);
        }
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

    /// `PinPanel.rotate`'s pixels: turn an encoded PNG by `quarters` quarter
    /// turns (positive = clockwise) and re-encode it; returns the new size.
    pub fn rotate_png(png: &[u8], quarters: i64) -> Option<(i64, i64, Vec<u8>)> {
        let data = NSData::with_bytes(png);
        let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)?;
        let img = rep.CGImage()?;
        let (w, h) = (CGImage::width(Some(&img)) as f64, CGImage::height(Some(&img)) as f64);
        let q = quarters.rem_euclid(4);
        if q == 0 {
            return Some((w as i64, h as i64, png.to_vec()));
        }
        let (nw, nh) = if q % 2 == 1 { (h, w) } else { (w, h) };
        let ctx = bitmap_context(nw as i64, nh as i64)?;
        let half = core::f64::consts::FRAC_PI_2;
        match q {
            1 => {
                CGContext::translate_ctm(Some(&ctx), 0.0, w);
                CGContext::rotate_ctm(Some(&ctx), -half);
            }
            2 => {
                CGContext::translate_ctm(Some(&ctx), w, h);
                CGContext::rotate_ctm(Some(&ctx), 2.0 * half);
            }
            _ => {
                CGContext::translate_ctm(Some(&ctx), h, 0.0);
                CGContext::rotate_ctm(Some(&ctx), half);
            }
        }
        CGContext::draw_image(Some(&ctx), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)), Some(&img));
        let out = CGBitmapContextCreateImage(Some(&ctx))?;
        Some((nw as i64, nh as i64, encode_png(&out)?))
    }

    pub fn encode_png(image: &CGImage) -> Option<Vec<u8>> {
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
    use crate::ui::view_base;
    use std::cell::{Cell, RefCell};

    use objc2::rc::Retained;
    
    use objc2_app_kit::{NSApplication, NSTrackingArea, NSTrackingAreaOptions};
    use super::{ChromeRects, ShotModePill};
        use objc2_core_graphics::CGPath;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags, NSGraphicsContext, NSPanel,
        NSScreenSaverWindowLevel, NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
        NSWindowStyleMask,
    };
    use objc2_core_graphics::{CGColor, CGContext, CGPathDrawingMode};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    use super::{
        selection_handles, shot_cg, ButtonRing, PanelLayout, Point, Rect, ShotColor,
        ShotHelpCard, ShotLoupe, ShotObject, ShotSaveCard, ShotSession, ShotSidePanel, ShotTool,
        ShotWheel,
    };
    use crate::engines::screenshot_annotations::ShotText;

    /// The blue the Swift overlay uses for text mode (`shotTextBlue`).
    const SHOT_TEXT_BLUE: ShotColor = ShotColor {
        // `shotTextBlue` = `ShotColor(hex: "#0A84FF")!` in `ScreenshotOverlay.swift`.
        r: 0x0a as f64 / 255.0,
        g: 0x84 as f64 / 255.0,
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
        Moved,
        Down,
        Dragged,
        Up,
        RightDown,
        RightDragged,
        RightUp,
    }

    pub struct ShotOverlayViewIvars {
        /// The live session. Owned by the controller; cleared before it drops.
        session: Cell<*mut ShotSession>,
        display: Cell<usize>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        /// Secure-pixelate pixels of the frozen capture, keyed by the image
        /// pointer (rasterized once per capture, not per frame).
        pixels: RefCell<Option<(usize, Option<crate::engines::screenshot_annotations::ShotPixels>)>>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShotOverlayView"]
        #[ivars = ShotOverlayViewIvars]
        pub struct ShotOverlayView;

        impl ShotOverlayView {
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.render();
            }

            #[unsafe(method(updateTrackingAreas))]
            fn update_tracking_areas(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect);
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, event: &NSEvent) {
                self.feed(event, MousePhase::Moved);
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

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, event: &NSEvent) {
                self.feed(event, MousePhase::RightDown);
            }

            #[unsafe(method(rightMouseDragged:))]
            fn right_mouse_dragged(&self, event: &NSEvent) {
                self.feed(event, MousePhase::RightDragged);
            }

            #[unsafe(method(rightMouseUp:))]
            fn right_mouse_up(&self, event: &NSEvent) {
                self.feed(event, MousePhase::RightUp);
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
                tracking: RefCell::new(None),
                pixels: RefCell::new(None),
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
                MousePhase::Moved => {
                    session.mouse_moved(d, p);
                    // The pill / loupe follow the pointer across displays.
                    if let Some(mtm) = MainThreadMarker::new() {
                        for w in NSApplication::sharedApplication(mtm).windows().iter() {
                            if let Some(p) = w.downcast_ref::<ShotOverlayPanel>() {
                                p.redraw();
                            }
                        }
                    }
                }
                MousePhase::RightDown => session.right_down(d, p),
                MousePhase::RightDragged => session.right_dragged(d, p),
                MousePhase::RightUp => session.right_up(d, p),
                MousePhase::Down => session.mouse_down(d, p, click, shift, cmd),
                MousePhase::Dragged => session.mouse_dragged(d, p, shift, cmd),
                MousePhase::Up => session.mouse_up(d, p, shift, cmd),
            }
            // A merge reorder swapped the canvas: reload the layer contents.
            if std::mem::take(&mut session.base_dirty) {
                if let Some(w) = self.window() {
                    if let Some(panel) = w.downcast_ref::<ShotOverlayPanel>() {
                        let img = session.base_images.get(d).cloned().flatten();
                        panel.set_base(img.as_deref());
                    }
                }
            }
            if matches!(
                phase,
                MousePhase::Down
                    | MousePhase::Up
                    | MousePhase::RightDown
                    | MousePhase::RightDragged
                    | MousePhase::RightUp
            ) {
                super::pump_after_mouse();
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

            let mine = if session.active == Some(d) {
                session.selection
            } else {
                None
            };

            // 1. The frozen capture is the panel container's layer (GPU
            //    composited, see `ShotOverlayPanel::create`). Redraw it here
            //    only inside the selection, where annotations draw: Multiply
            //    (marker) and Difference need real pixels under them. Outside
            //    it this view stays transparent, so a mouse move no longer
            //    re-blits the whole full-resolution capture.
            if let Some(sel) = mine.filter(|s| s.w > 0.0 && s.h > 0.0) {
                if let Some(img) = session.base_images.get(d).and_then(|b| b.as_deref()) {
                    // 1:1 with the backing store for a capture; a merge canvas
                    // can carry more pixels per point and needs filtering.
                    let backing = session.displays.get(d).map(|x| x.frame.w * x.scale).unwrap_or(w);
                    let smooth = (objc2_core_graphics::CGImage::width(Some(img)) as f64 - backing.round()).abs() > 1.0;
                    CGContext::save_g_state(Some(&ctx));
                    CGContext::clip_to_rect(Some(&ctx), cg_rect(sel));
                    shot_cg::draw_image(&ctx, img, full, smooth);
                    CGContext::restore_g_state(Some(&ctx));
                }
            }

            // 2. Dim outside the selection (even-odd bounds + selection) for
            //    the active display; a plain fill when the display is not active.
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

            // 3a. Copy Text mode (`ShotOverlayView.draw`): the whole screen gets
            //     the mode colour at 14% fill plus an 8pt frame at 0.6 alpha,
            //     inset 4pt. Swift fills `dirty` then redraws the base inside the
            //     selection, so the tint covers everything except the selection;
            //     an even-odd fill (full minus selection) is the same result.
            if session.text_mode {
                set_fill(&ctx, mode_color.with_alpha(0.14));
                match mine.filter(|s| s.w > 0.0 && s.h > 0.0) {
                    Some(sel) => {
                        CGContext::save_g_state(Some(&ctx));
                        CGContext::begin_path(Some(&ctx));
                        CGContext::add_rect(Some(&ctx), cg_rect(full));
                        CGContext::add_rect(Some(&ctx), cg_rect(sel));
                        CGContext::draw_path(Some(&ctx), CGPathDrawingMode::EOFill);
                        CGContext::restore_g_state(Some(&ctx));
                    }
                    None => CGContext::fill_rect(Some(&ctx), cg_rect(full)),
                }
                set_stroke(&ctx, mode_color.with_alpha(0.6));
                CGContext::set_line_width(Some(&ctx), 8.0);
                CGContext::stroke_rect(
                    Some(&ctx),
                    cg_rect(Rect::new(4.0, 4.0, full.w - 8.0, full.h - 8.0)),
                );
            }

            // 2b. The annotations, live (`ShotOverlayView.draw`): every object,
            //     the one being drawn, clipped to the selection; the object
            //     under a text edit is hidden while its draft draws (step 4).
            if let Some(sel) = mine.filter(|s| s.w > 0.0 && s.h > 0.0) {
                if let Some(base) = session.base_images.get(d).and_then(|b| b.as_deref()) {
                    let fw = session.displays.get(d).map(|x| x.frame.w).unwrap_or(w);
                    let scale = super::canvas_scale(objc2_core_graphics::CGImage::width(Some(base)) as i64, fw);
                    let hidden = session.editing.as_ref().and_then(|e| e.index);
                    let drawing = session.drawing();
                    let mut objs: Vec<&ShotObject> = session
                        .doc
                        .objects
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| Some(*i) != hidden)
                        .map(|(_, o)| o)
                        .collect();
                    if let Some(o) = &drawing {
                        objs.push(o);
                    }
                    if objs.iter().any(|o| o.tool == ShotTool::Pixelate && o.secure) {
                        let key = base as *const _ as usize;
                        let stale = self.ivars().pixels.borrow().as_ref().map(|(k, _)| *k) != Some(key);
                        if stale {
                            *self.ivars().pixels.borrow_mut() = Some((key, shot_cg::pixels_for(base)));
                        }
                    }
                    let cache = self.ivars().pixels.borrow();
                    let none = None;
                    let pixels = cache.as_ref().map(|(_, p)| p).unwrap_or(&none);
                    CGContext::save_g_state(Some(&ctx));
                    CGContext::clip_to_rect(Some(&ctx), cg_rect(sel));
                    shot_cg::draw_objects(&ctx, base, scale, pixels, &objs);
                    CGContext::restore_g_state(Some(&ctx));
                }
                // The selected object: a dashed box 4pt outside its bounds, plus
                // crisp resize handles for box/endpoint shapes so resizing is effortless.
                if let Some(o) = session.doc.selected.and_then(|i| session.doc.objects.get(i)) {
                    CGContext::save_g_state(Some(&ctx));
                    set_stroke(&ctx, cfg.ui_color);
                    CGContext::set_line_width(Some(&ctx), 1.0);
                    let dashes: [f64; 2] = [4.0, 3.0];
                    unsafe { CGContext::set_line_dash(Some(&ctx), 0.0, dashes.as_ptr(), dashes.len()) };
                    let b = o.bbox();
                    CGContext::stroke_rect(Some(&ctx), cg_rect(Rect::new(b.x - 4.0, b.y - 4.0, b.w + 8.0, b.h + 8.0)));
                    unsafe { CGContext::set_line_dash(Some(&ctx), 0.0, std::ptr::null(), 0) };
                    let handles = super::object_handles(o);
                    for pt in handles {
                        let hr = Rect::new(pt.x - 4.0, pt.y - 4.0, 8.0, 8.0);
                        set_fill(&ctx, ShotColor::WHITE);
                        CGContext::fill_rect(Some(&ctx), cg_rect(hr));
                        set_stroke(&ctx, cfg.ui_color);
                        CGContext::set_line_width(Some(&ctx), 1.5);
                        CGContext::stroke_rect(Some(&ctx), cg_rect(hr));
                    }
                    CGContext::restore_g_state(Some(&ctx));
                }
            }

            // 3. Selection border: text mode is a 2pt dashed border with no
            //    handles; otherwise the current 1px border + eight handles.
            if let Some(sel) = mine {
                if sel.w > 0.0 && sel.h > 0.0 {
                    set_stroke(&ctx, mode_color);
                    if session.text_mode {
                        CGContext::set_line_width(Some(&ctx), 2.0);
                        let dashes: [f64; 2] = [6.0, 4.0];
                        unsafe {
                            CGContext::set_line_dash(Some(&ctx), 0.0, dashes.as_ptr(), dashes.len())
                        };
                        CGContext::stroke_rect(
                            Some(&ctx),
                            cg_rect(Rect::new(sel.x - 1.0, sel.y - 1.0, sel.w + 2.0, sel.h + 2.0)),
                        );
                        unsafe { CGContext::set_line_dash(Some(&ctx), 0.0, std::ptr::null(), 0) };
                    } else {
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
            }

            // 3b. Display grid (panel option): overlay-only guide lines inside
            //     the selection every `grid_size` points; never rendered out.
            if let Some(sel) = mine {
                if session.state.grid && !session.text_mode && sel.w > 0.0 && sel.h > 0.0 {
                    draw_grid(&ctx, sel, session.state.grid_size as f64);
                }
            }

            // 4. The text draft while a text object is being edited (the one
            //    annotation the model exposes without the full renderer).
            if let Some(e) = &session.editing {
                if e.display == d {
                    shot_cg::draw_text(&ctx, &e.object);
                    // A dashed box so the text field is visible while typing.
                    CGContext::save_g_state(Some(&ctx));
                    set_stroke(&ctx, session.cfg.ui_color);
                    CGContext::set_line_width(Some(&ctx), 1.0);
                    let dashes: [f64; 2] = [4.0, 3.0];
                    unsafe { CGContext::set_line_dash(Some(&ctx), 0.0, dashes.as_ptr(), dashes.len()) };
                    let b = e.object.bbox();
                    let o = Rect::new(b.x - 4.0, b.y - 4.0, b.w + 8.0, b.h + 8.0);
                    CGContext::stroke_rect(Some(&ctx), cg_rect(o));
                    unsafe { CGContext::set_line_dash(Some(&ctx), 0.0, std::ptr::null(), 0) };
                    // Resize handles: corners + mid-edges.
                    for hy in [0.0, 0.5, 1.0] {
                        for hx in [0.0, 0.5, 1.0] {
                            if hx == 0.5 && hy == 0.5 {
                                continue;
                            }
                            let h = Rect::new(o.x + hx * o.w - 3.5, o.y + hy * o.h - 3.5, 7.0, 7.0);
                            set_fill(&ctx, ShotColor::WHITE);
                            CGContext::fill_rect(Some(&ctx), cg_rect(h));
                            CGContext::stroke_rect(Some(&ctx), cg_rect(h));
                        }
                    }
                    CGContext::restore_g_state(Some(&ctx));
                }
            }

            // 5. The chrome subviews (help card, button ring, colour wheel,
            //    loupe, save card), painted into this context.
            self.render_chrome(session, &ctx, full, d);
        }

        /// The overlay chrome subviews (`ScreenshotOverlay.swift`'s
        /// `ShotHelpCard` / `ShotButton` ring / `ShotWheelView` / `ShotLoupe` /
        /// `ShotSaveCard`), painted into the same context instead of as AppKit
        /// subviews. Layout comes from the pure models in the parent module.
        fn render_chrome(&self, session: &ShotSession, ctx: &CGContext, full: Rect, d: usize) {
            let cfg = &session.cfg;
            let mine = if session.active == Some(d) {
                session.selection
            } else {
                None
            };

            // Cmd+/ : every shortcut (any time); else the help card, centred
            // while nothing is selected.
            if session.shortcuts_shown {
                draw_rows_card(ctx, &cfg.shortcuts, cfg.ui_color, full, 0.90, 1.0);
            } else if cfg.show_help && session.selection.is_none() {
                draw_help_card(ctx, session, full, d);
            }

            // Mode pill (Copy Text | Screenshot) + the recents button beside
            // it, on the display the pointer is on.
            draw_mode_chrome(ctx, session, full, d);

            // Side panel (above the selection, below the ring) or, while it
            // is closed, the Tool Settings tab that opens it.
            if let Some(l) = session.panel_layout(d) {
                draw_side_panel(ctx, session, &l);
            } else if let Some(tab) = session.tool_tab_rect(d) {
                draw_tool_tab(ctx, session, tab);
            }

            // Merge editor: the numbered photos, drag to reorder.
            if let Some(l) = session.merge_strip(d) {
                draw_merge_strip(ctx, session, &l);
            }

            // Button ring: only around a live selection, not in text mode.
            if let Some(sel) = mine {
                if !session.text_mode {
                    draw_button_ring(ctx, session, d, sel);
                }
            }

            // Floating quick-style bar (color swatches, size, font, B/I).
            if let Some(qb) = session.quick_bar_layout(d) {
                draw_quick_bar(ctx, session, d, &qb);
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

            // Save card (the path field is edited through the key router, not
            // an AppKit field, and the card + value paint here).
            if let Some(card) = &session.save_card {
                if card.display == d {
                    draw_save_card(ctx, session, full, &card.field, card.sel);
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

    /// Draw an SF Symbol centred in `r` into the overlay's current graphics
    /// context (Swift: `NSImage(systemSymbolName:)` with a palette colour).
    fn draw_symbol(name: &str, r: Rect, point: f64, bold: bool, color: ShotColor) {
        use crate::ui::style::{symbol_image, Weight};
        use crate::ui::theme::Rgba;
        if name.is_empty() {
            return;
        }
        let w = if bold { Weight::Semibold } else { Weight::Medium };
        let Some(img) = symbol_image(name, point, w, Rgba::new(color.r, color.g, color.b, color.a)) else {
            return;
        };
        let sz = img.size();
        let dst = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(
                (r.mid_x() - sz.width / 2.0).round(),
                (r.mid_y() - sz.height / 2.0).round(),
            ),
            sz,
        );
        unsafe {
            img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                dst,
                objc2_foundation::NSRect::ZERO,
                objc2_app_kit::NSCompositingOperation::SourceOver,
                1.0,
                true,
                None,
            );
        }
    }

    /// `ShotModePill` + `ShotRecentButton`, laid out like `layoutChrome`:
    /// the pill is centred 16pt under the menu bar, the 28pt recents button
    /// 12pt to its right. Records the painted rects for the click hit test.
    fn draw_mode_chrome(ctx: &CGContext, session: &ShotSession, full: Rect, d: usize) {
        let shown = session.mouse.map(|(md, _)| md) == Some(d) && !session.is_dragging();
        if !shown {
            if session.chrome_rects.get().is_some_and(|c| c.display == d) {
                session.chrome_rects.set(None);
            }
            return;
        }
        let Some(display) = session.displays.get(d) else { return };
        let top = display.top_inset.max(24.0);
        let titles = ["Copy Text", "Screenshot"];
        let widths: Vec<f64> = titles
            .iter()
            .map(|t| shot_cg::measure_text_width(t, 16.0, true).ceil() + 18.0 + 8.0 + 28.0)
            .collect();
        let sum = widths.iter().sum::<f64>();
        // [4 pad][segments][hairline gap 11][chip 26][8 pad]
        let pw = sum + 4.0 + 11.0 + 26.0 + 8.0;
        let pill = Rect::new(ShotModePill::origin_x(full.w, pw), top + 16.0, pw, ShotModePill::HEIGHT);
        let recent = Rect::new(pill.x + 4.0 + sum + 11.0, pill.mid_y() - 13.0, 26.0, 26.0);
        session.chrome_rects.set(Some(ChromeRects {
            display: d,
            pill,
            first_w: widths[0],
            second_w: widths[1],
            recent,
        }));
        let in_recent = session.recent_open;

        let rounded = |r: Rect, rad: f64| unsafe {
            let path = CGPath::with_rounded_rect(cg_rect(r), rad, rad, std::ptr::null());
            CGContext::add_path(Some(ctx), Some(&*path));
        };
        // Pill body + hairline border.
        set_fill(ctx, ShotColor::BLACK.with_alpha(0.72));
        rounded(pill, pill.h / 2.0);
        CGContext::fill_path(Some(ctx));
        set_stroke(ctx, ShotColor::WHITE.with_alpha(0.25));
        CGContext::set_line_width(Some(ctx), 1.0);
        rounded(Rect::new(pill.x + 0.5, pill.y + 0.5, pill.w - 1.0, pill.h - 1.0), pill.h / 2.0);
        CGContext::stroke_path(Some(ctx));
        // Segments: the active one is filled (blue for text mode). While the
        // recents grid is the active stop, neither mode is lit.
        let mut x = pill.x + 4.0;
        for (i, (title, w)) in titles.iter().zip(widths.iter()).enumerate() {
            let r = Rect::new(x, pill.y + 4.0, *w, pill.h - 8.0);
            let on = !in_recent && (i == 0) == session.text_mode;
            if on {
                set_fill(ctx, if i == 0 { SHOT_TEXT_BLUE } else { session.cfg.ui_color });
                rounded(r, r.h / 2.0);
                CGContext::fill_path(Some(ctx));
            }
            let fg = if on { ShotColor::WHITE } else { ShotColor::WHITE.with_alpha(0.75) };
            let tw = shot_cg::measure_text_width(title, 16.0, true);
            let content = 18.0 + 8.0 + tw;
            let x0 = r.x + (r.w - content) / 2.0;
            draw_symbol(
                if i == 0 { "text.viewfinder" } else { "camera" },
                Rect::new(x0, r.y, 18.0, r.h),
                16.0,
                true,
                fg,
            );
            draw_text_at(ctx, title, x0 + 18.0 + 8.0, r.mid_y() - 10.0, 16.0, fg, true);
            x += *w;
        }
        // Hairline between the modes and the recents chip.
        set_fill(ctx, ShotColor::WHITE.with_alpha(0.22));
        CGContext::fill_rect(
            Some(ctx),
            cg_rect(Rect::new(pill.x + 4.0 + sum + 4.0, pill.mid_y() - 9.0, 1.0, 18.0)),
        );
        // Recents chip: smaller, same clock glyph as the standalone button.
        if in_recent {
            set_fill(ctx, ShotColor::WHITE);
        } else {
            set_fill(ctx, ShotColor::WHITE.with_alpha(0.08));
        }
        CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(recent));
        set_stroke(ctx, ShotColor::WHITE.with_alpha(if in_recent { 1.0 } else { 0.25 }));
        CGContext::set_line_width(Some(ctx), 1.0);
        CGContext::stroke_ellipse_in_rect(
            Some(ctx),
            cg_rect(Rect::new(recent.x + 0.5, recent.y + 0.5, recent.w - 1.0, recent.h - 1.0)),
        );
        let glyph = if in_recent { ShotColor::BLACK } else { ShotColor::WHITE.with_alpha(0.85) };
        draw_symbol("clock.arrow.circlepath", recent, 13.0, true, glyph);
    }

    /// `ShotHelpCard`: a translucent centred rounded card of key/action rows.
    /// Fades almost completely out when the pointer approaches it so it never
    /// blocks an item the user is trying to select.
    fn draw_help_card(ctx: &CGContext, session: &ShotSession, full: Rect, d: usize) {
        let cfg = &session.cfg;
        let rows: &[(String, String)] = if session.text_mode {
            &cfg.help_text_rows
        } else {
            &cfg.help_rows
        };
        if rows.is_empty() || cfg.help_opacity <= 0.01 {
            return;
        }
        let ui = if session.text_mode {
            SHOT_TEXT_BLUE
        } else {
            cfg.ui_color
        };
        let (size, o) = rows_card_frame(rows, full);
        let card_rect = Rect::new(o.x, o.y, size.w, size.h);
        let near_pointer = session
            .mouse
            .is_some_and(|(md, mp)| md == d && card_rect.inset(-40.0, -40.0).contains_point(mp));
        let fade = if near_pointer { 0.14 } else { 1.0 };
        draw_rows_card(ctx, rows, ui, full, cfg.help_opacity, fade);
    }

    fn rows_card_frame(rows: &[(String, String)], full: Rect) -> (super::Size, Point) {
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
        (size, ShotHelpCard::centered_origin(full, size))
    }

    /// A centred rounded card of `key  action` rows (the help card, the Cmd+/ list).
    fn draw_rows_card(
        ctx: &CGContext,
        rows: &[(String, String)],
        ui: ShotColor,
        full: Rect,
        bg_alpha: f64,
        fade: f64,
    ) {
        if rows.is_empty() {
            return;
        }
        let (point, bold) = (14.0, 14.0);
        let key_w = rows
            .iter()
            .map(|(k, _)| shot_cg::measure_text_width(k, bold, true))
            .fold(0.0f64, f64::max);
        let (size, o) = rows_card_frame(rows, full);
        let r = Rect::new(o.x, o.y, size.w, size.h);
        let fg = if ui.is_dark() || bg_alpha < 0.7 {
            ShotColor::WHITE
        } else {
            ShotColor::BLACK
        };
        let fill_a = (bg_alpha * fade).clamp(0.0, 1.0);
        let border_a = (0.48 * fade).clamp(0.0, 1.0);
        let text_a = (0.94 * fade).clamp(0.0, 1.0);
        fill_round(ctx, r, 10.0, ui.with_alpha(fill_a));
        stroke_round(ctx, r, 10.0, ShotColor::WHITE.with_alpha(border_a), 1.0);
        let mut y = o.y + 16.0;
        for (k, a) in rows {
            let kw = shot_cg::measure_text_width(k, bold, true);
            draw_text_at(ctx, k, o.x + 24.0 + key_w - kw, y, bold, fg.with_alpha(text_a), true);
            draw_text_at(ctx, a, o.x + 24.0 + key_w + 18.0, y, point, fg.with_alpha(text_a), false);
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
        // The discs are filled here; their SF Symbol glyphs are painted per
        // frame by `draw_symbol` below (and the size badge by its number).
        for (t, f) in cfg.buttons.iter().zip(layout.frames.iter()) {
            let c = Point::new(f.mid_x(), f.mid_y());
            let r = (f.w / 2.0 - 0.5).max(1.0);
            let active = session.tool == Some(*t) || (*t == ShotTool::Move && session.move_mode);
            let hover = session.mouse.is_some_and(|(md, mp)| md == d && f.contains_point(mp));
            // Copy is the main action (Return does the same): drawn
            // inverted so it stands out from the other discs.
            let primary = *t == ShotTool::Copy;
            let fill = if active {
                cfg.ui_color.mixed(&ShotColor::BLACK, 0.25)
            } else if primary {
                ShotColor::WHITE.mixed(&cfg.ui_color, if hover { 0.18 } else { 0.0 })
            } else if hover {
                cfg.ui_color.mixed(&ShotColor::WHITE, 0.18)
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
            if *t != ShotTool::Size {
                let on_ui = if cfg.ui_color.is_dark() { ShotColor::WHITE } else { ShotColor::BLACK };
                let fg = if primary && !active { cfg.ui_color } else { on_ui };
                draw_symbol(t.symbol(), *f, f.h * 0.42, false, fg);
            }
            // The tool's key, on a small badge at the disc's lower right.
            if let Some(letter) = t.letter() {
                let br = (f.w * 0.2).clamp(6.0, 9.0);
                let bc = Point::new(f.max_x() - br * 0.6, f.max_y() - br * 0.6);
                set_fill(ctx, cfg.contrast_color);
                CGContext::fill_ellipse_in_rect(
                    Some(ctx),
                    cg_rect(Rect::new(bc.x - br, bc.y - br, 2.0 * br, 2.0 * br)),
                );
                let label = letter.to_ascii_uppercase().to_string();
                let pt = (br * 1.25).round();
                let lw = shot_cg::measure_text_width(&label, pt, true);
                draw_text_at(ctx, &label, bc.x - lw / 2.0, bc.y - pt * 0.62, pt, ShotColor::WHITE, true);
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
        // The hovered button's name + key, beside it (the ring is icon-only).
        let hovered = session.mouse.and_then(|(md, mp)| {
            (md == d)
                .then(|| cfg.buttons.iter().zip(layout.frames.iter()).find(|(_, f)| f.contains_point(mp)))
                .flatten()
        });
        if let Some((t, f)) = hovered {
            draw_button_tip(ctx, t.tooltip(), *f, sel, display.bounds(), cfg.contrast_color);
        }
    }

    /// One tooltip pill next to a ring button, on the side away from the
    /// selection, kept on screen.
    fn draw_button_tip(ctx: &CGContext, text: &str, f: Rect, sel: Rect, bounds: Rect, bg: ShotColor) {
        if text.is_empty() {
            return;
        }
        let pt = 12.0;
        let w = shot_cg::measure_text_width(text, pt, false) + 16.0;
        let h = 22.0;
        let mut x = f.mid_x() - w / 2.0;
        let mut y = if f.min_y() >= sel.max_y() { f.max_y() + 6.0 } else { f.min_y() - h - 6.0 };
        if y < bounds.min_y() {
            y = f.max_y() + 6.0;
        }
        if y + h > bounds.max_y() {
            y = f.min_y() - h - 6.0;
        }
        x = x.clamp(bounds.min_x() + 4.0, (bounds.max_x() - w - 4.0).max(bounds.min_x() + 4.0));
        set_fill(ctx, bg.with_alpha(0.95));
        // SAFETY: a null transform is allowed (identity).
        let path = unsafe { CGPath::with_rounded_rect(cg_rect(Rect::new(x, y, w, h)), 6.0, 6.0, std::ptr::null()) };
        CGContext::add_path(Some(ctx), Some(&*path));
        CGContext::fill_path(Some(ctx));
        draw_text_at(ctx, text, x + 8.0, y + 3.0, pt, ShotColor::WHITE, false);
    }

    /// Floating quick-style bar: one-click color swatches, size/font-size
    /// stepper, and (for Text) font family + Bold/Italic pills.
    fn draw_quick_bar(ctx: &CGContext, session: &ShotSession, d: usize, l: &super::QuickBarLayout) {
        let cfg = &session.cfg;
        let ui = cfg.ui_color;
        let cur = session.current_color();
        let hover_at = |r: Rect| session.mouse.is_some_and(|(md, mp)| md == d && r.contains_point(mp));

        // Pill body + crisp hairline border.
        let bg = cfg.contrast_color.mixed(&ShotColor::BLACK, 0.55).with_alpha(0.92);
        fill_round(ctx, l.bar, l.bar.h / 2.0, bg);
        stroke_round(ctx, l.bar, l.bar.h / 2.0, ShotColor::WHITE.with_alpha(0.24), 1.0);

        // Color swatches.
        for (r, c) in &l.swatches {
            let active = cur.hex().eq_ignore_ascii_case(&c.hex());
            let hover = hover_at(r.inset(-2.0, -2.0));
            let draw_r = if hover { r.inset(-1.0, -1.0) } else { *r };
            set_fill(ctx, *c);
            CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(draw_r));
            if active {
                set_stroke(ctx, ShotColor::WHITE);
                CGContext::set_line_width(Some(ctx), 2.0);
                CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(draw_r.inset(-1.5, -1.5)));
            } else {
                set_stroke(ctx, ShotColor::WHITE.with_alpha(0.45));
                CGContext::set_line_width(Some(ctx), 1.0);
                CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(draw_r));
            }
        }

        // Optional custom color picker button (toggles side panel).
        if let Some(pr) = l.picker {
            let fill = if session.side_panel_open {
                ui
            } else if hover_at(pr) {
                ShotColor::WHITE.with_alpha(0.24)
            } else {
                ShotColor::WHITE.with_alpha(0.12)
            };
            set_fill(ctx, fill);
            CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(pr));
            set_stroke(ctx, ShotColor::WHITE.with_alpha(0.4));
            CGContext::set_line_width(Some(ctx), 1.0);
            CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(pr));
            draw_symbol("paintpalette", pr, 11.0, false, ShotColor::WHITE);
        }

        // Divider before size stepper.
        if !l.swatches.is_empty() || l.picker.is_some() {
            let dx = (l.size_down.x - 6.0).floor() + 0.5;
            fill_rect(ctx, Rect::new(dx, l.bar.y + 8.0, 1.0, l.bar.h - 16.0), ShotColor::WHITE.with_alpha(0.22));
        }

        // Size / font-size stepper: [-] [16pt] [+]
        for (r, glyph) in [(l.size_down, "\u{2212}"), (l.size_up, "+")] {
            let fill = if hover_at(r) {
                ui.mixed(&ShotColor::WHITE, 0.18)
            } else {
                ShotColor::WHITE.with_alpha(0.14)
            };
            fill_round(ctx, r, 5.0, fill);
            draw_text_centered(ctx, glyph, r, 14.0, ShotColor::WHITE, true);
        }
        let size_txt = session.quick_size_label().unwrap_or_default();
        draw_text_centered(ctx, &size_txt, l.size_value, 12.0, ShotColor::WHITE, true);

        // Text-specific controls: Font family pill + Bold + Italic.
        if let (Some(fr), Some(br), Some(ir)) = (l.font, l.bold, l.italic) {
            let dx = (fr.x - 6.0).floor() + 0.5;
            fill_rect(ctx, Rect::new(dx, l.bar.y + 8.0, 1.0, l.bar.h - 16.0), ShotColor::WHITE.with_alpha(0.22));
            let st = session.panel_text_style();
            let font_label = super::font_preset_label(&st.family);
            let fbg = if hover_at(fr) {
                ui.mixed(&ShotColor::WHITE, 0.18)
            } else {
                ShotColor::WHITE.with_alpha(0.14)
            };
            fill_round(ctx, fr, 5.0, fbg);
            draw_text_centered(ctx, font_label, fr, 11.5, ShotColor::WHITE, true);

            for (r, label, on, is_bold) in [(br, "B", st.bold, true), (ir, "I", st.italic, false)] {
                let bg_c = if on {
                    ui
                } else if hover_at(r) {
                    ShotColor::WHITE.with_alpha(0.24)
                } else {
                    ShotColor::WHITE.with_alpha(0.12)
                };
                fill_round(ctx, r, 5.0, bg_c);
                draw_text_centered(ctx, label, r, 12.0, ShotColor::WHITE, is_bold);
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

    fn round_path(ctx: &CGContext, r: Rect, rad: f64) {
        unsafe {
            let path = CGPath::with_rounded_rect(cg_rect(r), rad, rad, std::ptr::null());
            CGContext::add_path(Some(ctx), Some(&*path));
        }
    }

    fn fill_round(ctx: &CGContext, r: Rect, rad: f64, c: ShotColor) {
        set_fill(ctx, c);
        round_path(ctx, r, rad);
        CGContext::fill_path(Some(ctx));
    }

    fn stroke_round(ctx: &CGContext, r: Rect, rad: f64, c: ShotColor, width: f64) {
        set_stroke(ctx, c);
        CGContext::set_line_width(Some(ctx), width);
        round_path(ctx, Rect::new(r.x + 0.5, r.y + 0.5, r.w - 1.0, r.h - 1.0), rad);
        CGContext::stroke_path(Some(ctx));
    }

    /// Text centred in `r` (one line).
    fn draw_text_centered(ctx: &CGContext, text: &str, r: Rect, point: f64, c: ShotColor, bold: bool) {
        let w = shot_cg::measure_text_width(text, point, bold);
        draw_text_at(ctx, text, r.mid_x() - w / 2.0, r.mid_y() - point * 0.62, point, c, bold);
    }

    /// The display grid: hairlines every `step` points across `sel`.
    fn draw_grid(ctx: &CGContext, sel: Rect, step: f64) {
        if step < 1.0 {
            return;
        }
        CGContext::save_g_state(Some(ctx));
        CGContext::clip_to_rect(Some(ctx), cg_rect(sel));
        set_stroke(ctx, ShotColor::WHITE.with_alpha(0.28));
        CGContext::set_line_width(Some(ctx), 1.0);
        let mut x = sel.x + step;
        while x < sel.max_x() {
            CGContext::move_to_point(Some(ctx), x.floor() + 0.5, sel.y);
            CGContext::add_line_to_point(Some(ctx), x.floor() + 0.5, sel.max_y());
            x += step;
        }
        let mut y = sel.y + step;
        while y < sel.max_y() {
            CGContext::move_to_point(Some(ctx), sel.x, y.floor() + 0.5);
            CGContext::add_line_to_point(Some(ctx), sel.max_x(), y.floor() + 0.5);
            y += step;
        }
        CGContext::stroke_path(Some(ctx));
        CGContext::restore_g_state(Some(ctx));
    }

    /// The vertical "Tool Settings" tab hugging the left edge.
    fn draw_tool_tab(ctx: &CGContext, session: &ShotSession, tab: Rect) {
        let cfg = &session.cfg;
        let hover = session.mouse.is_some_and(|(_, p)| tab.contains_point(p));
        let fill = if hover { cfg.ui_color.mixed(&ShotColor::WHITE, 0.18) } else { cfg.ui_color };
        // Square on the screen edge, rounded on the inner side.
        fill_round(ctx, Rect::new(tab.x - 8.0, tab.y, tab.w + 8.0, tab.h), 7.0, fill.with_alpha(0.94));
        let fg = if cfg.ui_color.is_dark() { ShotColor::WHITE } else { ShotColor::BLACK };
        let label = &cfg.panel_labels.tab;
        let point = 12.0;
        let w = shot_cg::measure_text_width(label, point, true);
        // Rotate -90° so the label reads bottom-to-top along the tab.
        CGContext::save_g_state(Some(ctx));
        CGContext::translate_ctm(Some(ctx), tab.x + 3.0, tab.mid_y() + w / 2.0);
        CGContext::rotate_ctm(Some(ctx), -std::f64::consts::FRAC_PI_2);
        draw_text_at(ctx, label, 0.0, 0.0, point, fg, true);
        CGContext::restore_g_state(Some(ctx));
    }

    /// A `[-] value [+]` spin box.
    fn draw_spinner(ctx: &CGContext, session: &ShotSession, down: Rect, value: Rect, up: Rect, text: &str, enabled: bool) {
        let ui = session.cfg.ui_color;
        let a = if enabled { 1.0 } else { 0.35 };
        for (r, glyph) in [(down, "\u{2212}"), (up, "+")] {
            let hover = enabled && session.mouse.is_some_and(|(_, p)| r.contains_point(p));
            let fill = if hover { ui.mixed(&ShotColor::WHITE, 0.18) } else { ui };
            fill_round(ctx, r, 6.0, fill.with_alpha(a));
            draw_text_centered(ctx, glyph, r, 16.0, ShotColor::WHITE.with_alpha(a), true);
        }
        fill_round(ctx, value, 6.0, ShotColor::BLACK.with_alpha(0.35));
        stroke_round(ctx, value, 6.0, ShotColor::WHITE.with_alpha(0.25 * a), 1.0);
        draw_text_centered(ctx, text, value, 13.0, ShotColor::WHITE.with_alpha(a), true);
    }

    /// The hue/saturation disc: rings of wedges, outer (saturated) first.
    fn draw_hs_wheel(ctx: &CGContext, wheel: Rect) {
        let r = wheel.w.min(wheel.h) / 2.0;
        let (cx, cy) = (wheel.mid_x(), wheel.mid_y());
        let wedges = 60;
        let rings = 10;
        let tau = 2.0 * std::f64::consts::PI;
        for ring in (1..=rings).rev() {
            let rr = r * ring as f64 / rings as f64;
            let sat = ring as f64 / rings as f64;
            for w in 0..wedges {
                let a0 = tau * w as f64 / wedges as f64;
                let a1 = tau * (w + 1) as f64 / wedges as f64 + 0.01;
                let h = (w as f64 + 0.5) / wedges as f64;
                set_fill(ctx, ShotColor::from_hsv(h, sat, 1.0));
                CGContext::move_to_point(Some(ctx), cx, cy);
                CGContext::add_arc(Some(ctx), cx, cy, rr, a0, a1, 0);
                CGContext::close_path(Some(ctx));
                CGContext::fill_path(Some(ctx));
            }
        }
        set_fill(ctx, ShotColor::WHITE);
        let c = r / rings as f64;
        CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(Rect::new(cx - c, cy - c, 2.0 * c, 2.0 * c)));
        set_stroke(ctx, ShotColor::WHITE.with_alpha(0.5));
        CGContext::set_line_width(Some(ctx), 1.0);
        CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r)));
    }

    /// Flameshot's sidebar: size (label + spin box + slider), active colour,
    /// Grab Color, the colour wheel, the hex field and Display grid.
    fn draw_side_panel(ctx: &CGContext, session: &ShotSession, l: &PanelLayout) {
        let cfg = &session.cfg;
        let labels = &cfg.panel_labels;
        let ui = cfg.ui_color;
        let fg = ShotColor::WHITE;
        // Body: the contrast colour, darkened, nearly opaque; a ui-colour edge.
        fill_rect(ctx, l.panel, cfg.contrast_color.mixed(&ShotColor::BLACK, 0.45).with_alpha(0.95));
        fill_rect(ctx, Rect::new(l.panel.max_x() - 1.0, l.panel.y, 1.0, l.panel.h), ui);

        // Active tool size.
        let size = session.panel_size();
        let size_text = match size {
            Some((v, _, _)) => format!("{} {v}", labels.size),
            None => format!("{} \u{2014}", labels.size),
        };
        draw_text_at(ctx, &size_text, l.size_label.x, l.size_label.y, 13.0, fg, true);
        let value = size.map(|(v, _, _)| v.to_string()).unwrap_or_else(|| labels.no_tool.clone());
        draw_spinner(ctx, session, l.size_down, l.size_value, l.size_up, &value, size.is_some());
        let track = Rect::new(l.slider.x, l.slider.mid_y() - 2.0, l.slider.w, 4.0);
        fill_round(ctx, track, 2.0, ShotColor::WHITE.with_alpha(0.22));
        if let Some((v, lo, hi)) = size {
            let x = ShotSidePanel::slider_x(l.slider, v, lo, hi);
            fill_round(ctx, Rect::new(track.x, track.y, x - track.x, track.h), 2.0, ui.mixed(&ShotColor::WHITE, 0.2));
            set_fill(ctx, ShotColor::WHITE);
            CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(Rect::new(x - 7.0, l.slider.mid_y() - 7.0, 14.0, 14.0)));
            set_stroke(ctx, ui);
            CGContext::set_line_width(Some(ctx), 2.0);
            CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(Rect::new(x - 6.0, l.slider.mid_y() - 6.0, 12.0, 12.0)));
        }

        // Active colour.
        let cur = session.current_color();
        draw_text_at(ctx, &labels.color, l.color_label.x, l.color_label.y + 3.0, 13.0, fg, true);
        fill_round(ctx, l.swatch, 5.0, cur);
        stroke_round(ctx, l.swatch, 5.0, ShotColor::WHITE.with_alpha(0.8), 1.0);

        // Grab Color.
        let hover = session.mouse.is_some_and(|(_, p)| l.grab.contains_point(p));
        fill_round(ctx, l.grab, 7.0, if hover { ui.mixed(&ShotColor::WHITE, 0.18) } else { ui });
        draw_text_centered(ctx, &labels.grab, l.grab, 13.0, fg, true);

        // Wheel + the current colour's marker.
        draw_hs_wheel(ctx, l.wheel);
        let m = ShotSidePanel::wheel_point(l.wheel, cur);
        set_fill(ctx, cur);
        CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(Rect::new(m.x - 6.0, m.y - 6.0, 12.0, 12.0)));
        set_stroke(ctx, if cur.is_dark() { ShotColor::WHITE } else { ShotColor::BLACK });
        CGContext::set_line_width(Some(ctx), 2.0);
        CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(Rect::new(m.x - 6.0, m.y - 6.0, 12.0, 12.0)));

        // Hex field: focused = ui-colour ring, selected text highlighted.
        let editing = session.hex_edit.as_ref();
        fill_round(ctx, l.hex, 6.0, ShotColor::BLACK.with_alpha(0.45));
        match editing {
            Some(_) => stroke_round(ctx, l.hex, 6.0, ui.mixed(&ShotColor::WHITE, 0.3), 2.0),
            None => stroke_round(ctx, l.hex, 6.0, ShotColor::WHITE.with_alpha(0.3), 1.0),
        }
        let text = editing.map(|h| h.text.clone()).unwrap_or_else(|| cur.hex());
        if editing.is_some_and(|h| h.replace) && !text.is_empty() {
            let w = shot_cg::measure_text_width(&text, 13.0, false);
            fill_round(ctx, Rect::new(l.hex.mid_x() - w / 2.0 - 2.0, l.hex.y + 4.0, w + 4.0, l.hex.h - 8.0), 3.0, ui.with_alpha(0.7));
        }
        draw_text_centered(ctx, &text, l.hex, 13.0, fg, false);
        if editing.is_some_and(|h| !h.replace) {
            let w = shot_cg::measure_text_width(&text, 13.0, false);
            fill_rect(ctx, Rect::new(l.hex.mid_x() + w / 2.0 + 1.0, l.hex.y + 5.0, 1.5, l.hex.h - 10.0), fg);
        }

        // Display grid: checkbox + label, then the size spin box.
        let on = session.state.grid;
        let bx = Rect::new(l.grid_box.x, l.grid_box.y + 1.0, 18.0, 18.0);
        if on {
            fill_round(ctx, bx, 4.0, ui);
            draw_text_centered(ctx, "\u{2713}", bx, 13.0, fg, true);
        } else {
            stroke_round(ctx, bx, 4.0, ShotColor::WHITE.with_alpha(0.7), 1.5);
        }
        draw_text_at(ctx, &labels.grid, bx.max_x() + 8.0, l.grid_box.y + 1.0, 13.0, fg, true);
        let gs = session.state.grid_size.to_string();
        draw_spinner(ctx, session, l.grid_down, l.grid_value, l.grid_up, &gs, on);

        // Text: B I U S toggles + alignment, live only for text.
        let text_on = session.text_section_shown();
        let dim = |c: ShotColor| if text_on { c } else { c.with_alpha(0.35) };
        draw_text_at(ctx, &labels.text, l.text_label.x, l.text_label.y, 13.0, dim(fg), true);
        let st = session.panel_text_style();
        let toggle = |r: Rect, on: bool| {
            fill_round(ctx, r, 6.0, if on && text_on { ui } else { ShotColor::WHITE.with_alpha(if text_on { 0.12 } else { 0.06 }) });
            stroke_round(ctx, r, 6.0, ShotColor::WHITE.with_alpha(if text_on { 0.3 } else { 0.12 }), 1.0);
        };
        for (i, r) in l.styles.iter().enumerate() {
            toggle(*r, ShotSession::style_flag(&st, i));
            draw_text_centered(ctx, ["B", "I", "U", "S"][i], *r, 13.0, dim(fg), i == 0);
        }
        for (i, r) in l.aligns.iter().enumerate() {
            toggle(*r, st.align == i as i64);
            draw_symbol(["text.alignleft", "text.aligncenter", "text.alignright"][i], *r, 13.0, false, dim(fg));
        }

        // Layers: topmost first, the selected row highlighted.
        draw_text_at(ctx, &labels.layers, l.layers_label.x, l.layers_label.y, 13.0, fg, true);
        fill_round(ctx, l.layers, 6.0, ShotColor::BLACK.with_alpha(0.35));
        stroke_round(ctx, l.layers, 6.0, ShotColor::WHITE.with_alpha(0.2), 1.0);
        let n = session.doc.objects.len();
        let rows = (l.layers.h / ShotSidePanel::LAYER_ROW).floor() as usize;
        for r in 0..n.min(rows) {
            let i = n - 1 - r;
            let row = Rect::new(l.layers.x + 2.0, l.layers.y + r as f64 * ShotSidePanel::LAYER_ROW + 1.0, l.layers.w - 4.0, ShotSidePanel::LAYER_ROW - 2.0);
            if session.doc.selected == Some(i) {
                fill_round(ctx, row, 4.0, ui);
            }
            let o = &session.doc.objects[i];
            let swatch = Rect::new(row.x + 6.0, row.mid_y() - 5.0, 10.0, 10.0);
            fill_round(ctx, swatch, 2.0, o.color);
            draw_text_at(ctx, &super::layer_title(i, o), swatch.max_x() + 8.0, row.y + 1.0, 12.0, fg, false);
        }
        if n == 0 {
            draw_text_at(ctx, "No annotations yet", l.layers.x + 10.0, l.layers.y + 6.0, 12.0, fg.with_alpha(0.5), false);
        }
        let has_sel = session.doc.selected.is_some_and(|i| i < n);
        for (r, sym) in [(l.layer_delete, "trash"), (l.layer_up, "arrow.up"), (l.layer_down, "arrow.down")] {
            fill_round(ctx, r, 6.0, ShotColor::WHITE.with_alpha(if has_sel { 0.14 } else { 0.06 }));
            draw_symbol(sym, r, 13.0, false, fg.with_alpha(if has_sel { 1.0 } else { 0.35 }));
        }
    }

    /// The merge editor's reorder strip: one row per slot (number disc +
    /// thumbnail). While a row is dragged the list shows the order it would
    /// drop into, numbers included, with the dragged photo outlined.
    pub(super) fn draw_merge_strip(ctx: &CGContext, session: &ShotSession, l: &super::MergeStripLayout) {
        let Some(m) = session.merge.as_ref() else { return };
        let cfg = &session.cfg;
        let ui = cfg.ui_color;
        let fg = ShotColor::WHITE;
        fill_round(ctx, l.strip, 10.0, cfg.contrast_color.mixed(&ShotColor::BLACK, 0.45).with_alpha(0.92));
        stroke_round(ctx, l.strip, 10.0, ui.with_alpha(0.6), 1.0);
        draw_text_centered(ctx, &cfg.merge.order_hint, l.hint, 11.0, fg.with_alpha(0.75), false);
        // ↓ Vertical | → Horizontal: the live stacking, click to switch.
        let [v, h] = l.direction;
        let pill = Rect::new(v.x, v.y, h.max_x() - v.x, v.h);
        fill_round(ctx, pill, 6.0, ShotColor::WHITE.with_alpha(0.08));
        let on = if m.horizontal { h } else { v };
        fill_round(ctx, Rect::new(on.x + 2.0, on.y + 2.0, on.w - 4.0, on.h - 4.0), 5.0, ui);
        draw_text_centered(ctx, &cfg.merge.vertical_label, v, 10.0, fg.with_alpha(if m.horizontal { 0.6 } else { 1.0 }), !m.horizontal);
        draw_text_centered(ctx, &cfg.merge.horizontal_label, h, 10.0, fg.with_alpha(if m.horizontal { 1.0 } else { 0.6 }), m.horizontal);
        let order = super::merge_preview_order(m.images.len(), m.drag);
        let dragged = m.drag.map(|(from, _)| from);
        for (slot, row) in l.rows.iter().enumerate() {
            let Some(&photo) = order.get(slot) else { continue };
            let Some(img) = m.images.get(photo) else { continue };
            let lifted = dragged == Some(photo);
            fill_round(ctx, *row, 6.0, if lifted { ui.with_alpha(0.35) } else { ShotColor::WHITE.with_alpha(0.08) });
            if lifted {
                stroke_round(ctx, *row, 6.0, ui, 2.0);
            }
            // Number disc on the left, in the stack's label colour.
            let d = 22.0f64.min(row.h - 6.0).max(14.0);
            let disc = Rect::new(row.x + 4.0, row.mid_y() - d / 2.0, d, d);
            set_fill(ctx, cfg.merge.label_color);
            CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(disc));
            let lc = cfg.merge.label_color;
            let ink = if 0.299 * lc.r + 0.587 * lc.g + 0.114 * lc.b > 0.6 { ShotColor::BLACK } else { ShotColor::WHITE };
            draw_text_centered(ctx, &(slot + 1).to_string(), disc, (d * 0.55).round(), ink, true);
            // Thumbnail: aspect-fit in the rest of the row.
            let area = Rect::new(disc.max_x() + 6.0, row.y + 4.0, row.max_x() - disc.max_x() - 10.0, row.h - 8.0);
            let (iw, ih) = (
                objc2_core_graphics::CGImage::width(Some(img)) as f64,
                objc2_core_graphics::CGImage::height(Some(img)) as f64,
            );
            if iw > 0.0 && ih > 0.0 && area.w > 0.0 && area.h > 0.0 {
                let k = (area.w / iw).min(area.h / ih);
                let (tw, th) = ((iw * k).round(), (ih * k).round());
                let t = Rect::new(area.x + ((area.w - tw) / 2.0).round(), area.y + ((area.h - th) / 2.0).round(), tw, th);
                shot_cg::draw_image(ctx, img, t, true);
                set_stroke(ctx, ShotColor::WHITE.with_alpha(0.35));
                CGContext::set_line_width(Some(ctx), 1.0);
                CGContext::stroke_rect(Some(ctx), cg_rect(Rect::new(t.x + 0.5, t.y + 0.5, t.w - 1.0, t.h - 1.0)));
            }
        }
    }

    /// `ShotSaveCard`: the save-as card frame, title, value and hint.
    fn draw_save_card(ctx: &CGContext, session: &ShotSession, full: Rect, path: &str, sel: Option<(usize, usize)>) {
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
        if let Some((a, b)) = sel.filter(|(a, b)| b > a) {
            let pre: String = path.chars().take(a).collect();
            let mid: String = path.chars().skip(a).take(b - a).collect();
            let x0 = field.x + 4.0 + super::shot_cg::measure_text_width(&pre, 12.0, false);
            let w = super::shot_cg::measure_text_width(&mid, 12.0, false);
            set_fill(ctx, cfg.ui_color.with_alpha(0.45));
            CGContext::fill_rect(Some(ctx), cg_rect(Rect::new(x0, field.y + 2.0, w, field.h - 4.0)));
        }
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
        /// The content view whose layer shows the frozen capture.
        container: RefCell<Option<Retained<NSView>>>,
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
                .set_ivars(ShotOverlayPanelIvars {
                    overlay: RefCell::new(None),
                    container: RefCell::new(None),
                });
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
            // Transparent until the frozen capture lands (the async Hyper+X
            // path shows the overlay over the live screen first), and never
            // part of a screen capture, so that capture sees what is under it.
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setSharingType(objc2_app_kit::NSWindowSharingType::None);
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
            // The frozen capture is the container's layer contents: the GPU
            // composites it, so `drawRect:` never re-blits the full-resolution
            // image (it did on every mouse move). The overlay view on top draws
            // the dim, chrome and annotations, plus the base pixels inside the
            // selection only (blend-mode tools need them; see `render`).
            let bounds = NSRect::new(NSPoint::new(0.0, 0.0), rect.size);
            let container = NSView::initWithFrame(NSView::alloc(mtm), bounds);
            container.setWantsLayer(true);
            let view = ShotOverlayView::create(mtm, display, session, rect.size);
            container.addSubview(&view);
            panel.setContentView(Some(&container));
            *panel.ivars().overlay.borrow_mut() = Some(view);
            *panel.ivars().container.borrow_mut() = Some(container);
            // SAFETY: `session` is live on the main thread (see `feed`).
            let base = unsafe { session.as_ref() }
                .and_then(|s| s.base_images.get(display).cloned().flatten());
            panel.set_base(base.as_deref());
            panel
        }

        /// Show `img` (the frozen capture) as the container layer's contents;
        /// `None` leaves the panel see-through (capture still in flight).
        pub fn set_base(&self, img: Option<&objc2_core_graphics::CGImage>) {
            let Some(container) = self.ivars().container.borrow().clone() else { return };
            let Some(layer) = container.layer() else { return };
            let obj = img.map(|i| unsafe {
                &*(i as *const objc2_core_graphics::CGImage as *const objc2::runtime::AnyObject)
            });
            unsafe { layer.setContents(obj) };
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
        NSBackingStoreType, NSBitmapImageRep, NSColor, NSDragOperation, NSDraggingContext,
        NSDraggingSession, NSDraggingSource, NSFloatingWindowLevel, NSGraphicsContext, NSPanel,
        NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
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
        /// Right-click → `PinPanel.menu()` (set by the controller).
        on_menu: RefCell<Option<std::rc::Rc<dyn Fn()>>>,
        /// Double-click → `PinPanel.closePin` (set by the controller).
        on_close: RefCell<Option<std::rc::Rc<dyn Fn()>>>,
        /// Scroll-wheel zoom factor → `PinPanel.wheel` (set by the controller).
        on_wheel: RefCell<Option<std::rc::Rc<dyn Fn(f64)>>>,
        /// Magnify pinch factor → `PinPanel.pinch` (set by the controller).
        on_magnify: RefCell<Option<std::rc::Rc<dyn Fn(f64)>>>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPinView"]
        #[ivars = WSPinViewIvars]
        pub struct WSPinView;

        impl WSPinView {
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

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, _e: &objc2_app_kit::NSEvent) {
                let cb = self.ivars().on_menu.borrow().clone();
                if let Some(cb) = cb {
                    cb();
                }
            }

            /// `PinView.mouseDown`: double-click closes, Cmd-drag drops the
            /// picture into another app, otherwise drag the window
            /// (`performDrag(with:)`).
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &objc2_app_kit::NSEvent) {
                if e.modifierFlags().contains(objc2_app_kit::NSEventModifierFlags::Command) {
                    self.drag_out(e);
                    return;
                }
                if e.clickCount() == 2 {
                    let cb = self.ivars().on_close.borrow().clone();
                    if let Some(cb) = cb {
                        cb();
                    }
                    return;
                }
                if let Some(w) = self.window() {
                    w.performWindowDragWithEvent(e);
                }
            }

            /// `PinView.scrollWheel`: hand the zoom factor to the controller.
            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, e: &objc2_app_kit::NSEvent) {
                let dy = if e.hasPreciseScrollingDeltas() {
                    e.scrollingDeltaY() / 10.0
                } else {
                    e.scrollingDeltaY()
                };
                if dy == 0.0 {
                    return;
                }
                let factor = if dy > 0.0 { 1.03 } else { 1.0 / 1.03 };
                let cb = self.ivars().on_wheel.borrow().clone();
                if let Some(cb) = cb {
                    cb(factor);
                }
            }

            /// `PinView.magnify`: hand the pinch factor to the controller.
            #[unsafe(method(magnify:))]
            fn magnify(&self, e: &objc2_app_kit::NSEvent) {
                let cb = self.ivars().on_magnify.borrow().clone();
                if let Some(cb) = cb {
                    cb(1.0 + e.magnification());
                }
            }
        }

        unsafe impl NSObjectProtocol for WSPinView {}

        unsafe impl NSDraggingSource for WSPinView {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn drag_mask(&self, _s: &NSDraggingSession, _ctx: NSDraggingContext) -> NSDragOperation {
                NSDragOperation::Copy
            }
        }
    );

    impl WSPinView {
        /// Cmd-drag: the picture as a PNG file (Slack, Mail, Finder, a
        /// browser upload take it), with its thumbnail under the pointer.
        fn drag_out(&self, e: &objc2_app_kit::NSEvent) {
            use objc2::runtime::ProtocolObject;
            use objc2_app_kit::{NSDraggingItem, NSImage, NSPasteboardWriting};
            use objc2_foundation::{NSArray, NSString, NSURL};
            let png = self
                .window()
                .and_then(|w| crate::ui::view_base::as_any(&*w).downcast_ref::<WSPinPanel>().map(|p| p.png()))
                .unwrap_or_default();
            if png.is_empty() {
                return;
            }
            let dir = std::env::temp_dir().join("kitchen-sink-drag");
            let _ = std::fs::create_dir_all(&dir);
            let name = format!("Screenshot {}.png", super::ShotFiles::expand("%F at %H.%M.%S"));
            let path = dir.join(name);
            if std::fs::write(&path, &png).is_err() {
                return;
            }
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> = ProtocolObject::from_retained(url);
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
            let b = self.bounds();
            let thumb = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png));
            let k = (160.0 / b.size.width.max(1.0)).min(1.0);
            let (tw, th) = (b.size.width * k, b.size.height * k);
            let at = crate::ui::view_base::mouse_point(self, e);
            let frame = NSRect::new(NSPoint::new(at.x - tw / 2.0, at.y - th / 2.0), NSSize::new(tw, th));
            if let Some(t) = thumb {
                unsafe { item.setDraggingFrame_contents(frame, Some(crate::ui::view_base::as_any(&*t))) };
            } else {
                item.setDraggingFrame(frame);
            }
            let arr = NSArray::from_retained_slice(&[item]);
            self.beginDraggingSessionWithItems_event_source(&arr, e, ProtocolObject::from_ref(self));
        }

        fn create(
            mtm: MainThreadMarker,
            image: Retained<CGImage>,
            size: NSSize,
        ) -> Retained<WSPinView> {
            let this = WSPinView::alloc(mtm).set_ivars(WSPinViewIvars {
                image: RefCell::new(Some(image)),
                on_menu: RefCell::new(None),
                on_close: RefCell::new(None),
                on_wheel: RefCell::new(None),
                on_magnify: RefCell::new(None),
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
        /// The pinned image's encoded PNG (copy / save / rotate source).
        png: RefCell<Vec<u8>>,
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
                .set_ivars(WSPinPanelIvars { view: RefCell::new(None), png: RefCell::new(Vec::new()) });
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
            // The silver preview border around the pinned image.
            crate::ui::preview_ring::attach(base, m, 0.0);
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

        pub fn set_on_menu(&self, cb: std::rc::Rc<dyn Fn()>) {
            if let Some(v) = self.ivars().view.borrow().as_ref() {
                *v.ivars().on_menu.borrow_mut() = Some(cb);
            }
        }

        /// `PinView.mouseDown` double-click → `PinPanel.closePin`.
        pub fn set_on_close(&self, cb: std::rc::Rc<dyn Fn()>) {
            if let Some(v) = self.ivars().view.borrow().as_ref() {
                *v.ivars().on_close.borrow_mut() = Some(cb);
            }
        }

        /// `PinView.scrollWheel` → `PinPanel.wheel` (zoom factor).
        pub fn set_on_wheel(&self, cb: std::rc::Rc<dyn Fn(f64)>) {
            if let Some(v) = self.ivars().view.borrow().as_ref() {
                *v.ivars().on_wheel.borrow_mut() = Some(cb);
            }
        }

        /// `PinView.magnify` → `PinPanel.pinch` (zoom factor).
        pub fn set_on_magnify(&self, cb: std::rc::Rc<dyn Fn(f64)>) {
            if let Some(v) = self.ivars().view.borrow().as_ref() {
                *v.ivars().on_magnify.borrow_mut() = Some(cb);
            }
        }

        /// `PinPanel.setZoom`: apply a model frame (origin + size, in points).
        pub fn apply_frame(&self, f: Rect) {
            let nf = NSRect::new(
                NSPoint::new(f.x, f.y),
                NSSize::new(f.w.max(1.0), f.h.max(1.0)),
            );
            self.setFrame_display(nf, true);
        }

        pub fn png(&self) -> Vec<u8> {
            self.ivars().png.borrow().clone()
        }

        pub fn set_png(&self, png: Vec<u8>) {
            *self.ivars().png.borrow_mut() = png;
        }

        /// `PinPanel.rotate`: swap in the turned image and resize the
        /// window around its centre (the image keeps its on-screen scale).
        pub fn show_rotated(&self, png: Vec<u8>, odd: bool) {
            let Some(cg) = decode_png(&png) else { return };
            self.set_png(png);
            if let Some(v) = self.ivars().view.borrow().as_ref() {
                *v.ivars().image.borrow_mut() = Some(cg);
                v.setNeedsDisplay(true);
            }
            if odd {
                let f = self.frame();
                // Margins are equal on every edge, so the frame just swaps.
                let (w, h) = (f.size.height, f.size.width);
                let c = NSPoint::new(f.origin.x + f.size.width / 2.0, f.origin.y + f.size.height / 2.0);
                let nf = NSRect::new(NSPoint::new((c.x - w / 2.0).round(), (c.y - h / 2.0).round()), NSSize::new(w, h));
                self.setFrame_display(nf, true);
            }
        }
    }
}

/// `ShotRecentPanel` / `ShotRecentView` (Screenshot.swift): the floating
/// "Recent screenshots" grid opened with ⌘R from the overlay. Click a cell
/// to reopen it as a pin; right-click for its menu.
#[cfg(target_os = "macos")]
pub mod recent_grid {
    use crate::ui::view_base;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSEvent, NSImage, NSPanel, NSTrackingArea, NSView,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    use super::ShotRecentEntry;
    use crate::ui::chrome::Rect;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::Rgba;

    /// Shift, Cmd or Ctrl held = picking screenshots to merge.
    const PICK_MODS: objc2_app_kit::NSEventModifierFlags = objc2_app_kit::NSEventModifierFlags(
        objc2_app_kit::NSEventModifierFlags::Shift.0
            | objc2_app_kit::NSEventModifierFlags::Command.0
            | objc2_app_kit::NSEventModifierFlags::Control.0,
    );

    pub const COLS: usize = 4;
    const CELL_W: f64 = 172.0;
    const CELL_H: f64 = 132.0;
    const GAP: f64 = 10.0;
    const PAD: f64 = 16.0;
    const HEAD_H: f64 = 40.0;

    /// `ShotRecentView.init`'s size for `n` entries.
    pub fn grid_size(n: usize) -> (f64, f64) {
        let cols = COLS as f64;
        let rows = n.div_ceil(COLS).max(1) as f64;
        let w = PAD * 2.0 + cols * CELL_W + (cols - 1.0) * GAP;
        let h = if n == 0 { HEAD_H + 70.0 } else { HEAD_H + PAD + rows.min(3.5) * (CELL_H + GAP) - GAP + PAD };
        (w, h)
    }

    /// `ShotRecentView.age(_:)`-style short age.
    pub fn age(at: i64, now: i64) -> String {
        let s = (now - at).max(0);
        match s {
            0..=59 => "just now".into(),
            60..=3599 => format!("{} min ago", s / 60),
            3600..=86399 => format!("{} h ago", s / 3600),
            86400..=172_799 => "yesterday".into(),
            _ => format!("{} d ago", s / 86400),
        }
    }

    pub type EntryCb = Rc<dyn Fn(ShotRecentEntry)>;
    pub type MergeCb = Rc<dyn Fn(Vec<ShotRecentEntry>, bool)>;

    /// The header hints for Shift-click merging (`[screenshot] merge-hint`,
    /// `merge-picked-hint`; `{}` = how many are picked).
    #[derive(Clone, Debug, Default)]
    pub struct MergeHints {
        pub idle: String,
        pub picked: String,
        /// The direction pill's labels.
        pub vertical: String,
        pub horizontal: String,
    }

    pub struct ViewIvars {
        entries: RefCell<Vec<ShotRecentEntry>>,
        thumbs: RefCell<HashMap<String, Retained<NSImage>>>,
        hover: Cell<Option<usize>>,
        scroll: Cell<f64>,
        ui: Cell<Rgba>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        pub on_open: RefCell<Option<EntryCb>>,
        pub on_menu: RefCell<Option<EntryCb>>,
        /// Shift/Cmd-clicked screenshots, in click order (1, 2, 3 …).
        picked: RefCell<Vec<String>>,
        hints: RefCell<MergeHints>,
        /// The direction pill: stack left to right instead of top to bottom.
        horizontal: Cell<bool>,
        /// Release Shift (or Return) with 2+ picked: merge them, in order.
        pub on_merge: RefCell<Option<MergeCb>>,
        /// Keys on the highlighted shot: Space pins, Cmd+C copies, Delete
        /// moves it to the Trash (Return opens it: `on_open`).
        pub on_pin: RefCell<Option<EntryCb>>,
        pub on_copy: RefCell<Option<EntryCb>>,
        pub on_trash: RefCell<Option<EntryCb>>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShotRecentView"]
        #[ivars = ViewIvars]
        pub struct ShotRecentView;

        impl ShotRecentView {
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool { true }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                // Backstop for a Shift release the panel never saw as
                // `flagsChanged:` (it was not key at the time).
                if !e.modifierFlags().intersects(PICK_MODS) {
                    self.merge_if_ready();
                }
                let p = view_base::mouse_point(self, e);
                let i = self.index_at(p.x, p.y);
                view_base::set_hover(self, &self.ivars().hover, i);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                view_base::set_hover(self, &self.ivars().hover, None);
            }

            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, e: &NSEvent) {
                let visible = self.bounds().size.height - HEAD_H;
                let n = self.ivars().entries.borrow().len();
                let rows = n.div_ceil(COLS) as f64;
                let content = rows * (CELL_H + GAP) - GAP + PAD * 2.0;
                let k = if e.hasPreciseScrollingDeltas() { 1.0 } else { 10.0 };
                let y = (self.ivars().scroll.get() - e.scrollingDeltaY() * k).clamp(0.0, (content - visible).max(0.0));
                self.ivars().scroll.set(y);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {}

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                for (seg, horizontal) in [(self.pill_rect(false), false), (self.pill_rect(true), true)] {
                    if seg.contains(p.x, p.y) {
                        self.set_horizontal(horizontal);
                        return;
                    }
                }
                let picking = e.modifierFlags().intersects(PICK_MODS);
                let Some(entry) = self.index_at(p.x, p.y).and_then(|i| self.ivars().entries.borrow().get(i).cloned()) else {
                    return;
                };
                if picking {
                    self.toggle_pick(&entry.path);
                    return;
                }
                let cb = self.ivars().on_open.borrow().clone();
                if let Some(cb) = cb {
                    cb(entry);
                }
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                if let Some(e) = self.index_at(p.x, p.y).and_then(|i| self.ivars().entries.borrow().get(i).cloned()) {
                    let cb = self.ivars().on_menu.borrow().clone();
                    if let Some(cb) = cb {
                        cb(e);
                    }
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for ShotRecentView {}
    );

    impl ShotRecentView {
        fn create(mtm: MainThreadMarker, entries: Vec<ShotRecentEntry>, ui: Rgba) -> Retained<Self> {
            let (w, h) = grid_size(entries.len());
            let thumbs = entries
                .iter()
                .filter_map(|e| style::image_file(&e.path).map(|i| (e.path.clone(), i)))
                .collect();
            let this = Self::alloc(mtm).set_ivars(ViewIvars {
                picked: RefCell::new(Vec::new()),
                hints: RefCell::new(MergeHints::default()),
                horizontal: Cell::new(false),
                on_merge: RefCell::new(None),
                entries: RefCell::new(entries),
                thumbs: RefCell::new(thumbs),
                hover: Cell::new(None),
                scroll: Cell::new(0.0),
                ui: Cell::new(ui),
                tracking: RefCell::new(None),
                on_open: RefCell::new(None),
                on_menu: RefCell::new(None),
                on_pin: RefCell::new(None),
                on_copy: RefCell::new(None),
                on_trash: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h))] }
        }

        pub fn set_hints(&self, hints: MergeHints) {
            *self.ivars().hints.borrow_mut() = hints;
            self.setNeedsDisplay(true);
        }

        /// Shift/Cmd-click: add `path` to the merge picks (next number), or
        /// drop it (the rest renumber).
        fn toggle_pick(&self, path: &str) {
            {
                let mut picked = self.ivars().picked.borrow_mut();
                match picked.iter().position(|p| p == path) {
                    Some(i) => {
                        picked.remove(i);
                    }
                    None => picked.push(path.to_string()),
                }
            }
            self.setNeedsDisplay(true);
        }

        /// The picks, in order; `true` when something was picked.
        pub fn has_picks(&self) -> bool {
            !self.ivars().picked.borrow().is_empty()
        }

        pub fn set_horizontal(&self, on: bool) {
            self.ivars().horizontal.set(on);
            self.setNeedsDisplay(true);
        }

        /// One half of the Vertical | Horizontal pill, at the header's right.
        fn pill_rect(&self, horizontal: bool) -> Rect {
            let seg = 92.0;
            let x = self.bounds().size.width - PAD - 2.0 * seg + if horizontal { seg } else { 0.0 };
            Rect::new(x, 9.0, seg, 24.0)
        }

        /// Restore picks (in order), e.g. after Esc out of the merge editor.
        pub fn set_picks(&self, paths: Vec<String>) {
            let known: Vec<String> = {
                let entries = self.ivars().entries.borrow();
                paths.into_iter().filter(|p| entries.iter().any(|e| &e.path == p)).collect()
            };
            *self.ivars().picked.borrow_mut() = known;
            self.setNeedsDisplay(true);
        }

        pub fn clear_picks(&self) {
            self.ivars().picked.borrow_mut().clear();
            self.setNeedsDisplay(true);
        }

        /// Hand 2+ picks to `on_merge` (in click order) and clear them. A
        /// single pick stays picked (Shift-click another to go on).
        pub fn merge_if_ready(&self) -> bool {
            let picks: Vec<ShotRecentEntry> = {
                let picked = self.ivars().picked.borrow();
                if picked.len() < 2 {
                    return false;
                }
                let entries = self.ivars().entries.borrow();
                picked
                    .iter()
                    .filter_map(|p| entries.iter().find(|e| &e.path == p).cloned())
                    .collect()
            };
            self.ivars().picked.borrow_mut().clear();
            self.setNeedsDisplay(true);
            if picks.len() < 2 {
                return false;
            }
            let cb = self.ivars().on_merge.borrow().clone();
            if let Some(cb) = cb {
                cb(picks, self.ivars().horizontal.get());
            }
            true
        }

        /// Arrows move the highlight (the first press lands on the newest
        /// shot); Return opens it in the editor (or merges 2+ picks);
        /// Space pins; Cmd+C copies; Delete moves it to the Trash.
        /// `false` = not a grid key.
        pub fn key(&self, e: &NSEvent) -> bool {
            use objc2_app_kit::NSEventModifierFlags as F;
            let n = self.ivars().entries.borrow().len();
            let flags = e.modifierFlags();
            let cmd = flags.contains(F::Command);
            let cur = self.ivars().hover.get();
            let entry = |i: Option<usize>| i.and_then(|i| self.ivars().entries.borrow().get(i).cloned());
            let fire = |slot: &RefCell<Option<EntryCb>>, i: Option<usize>| {
                let cb = slot.borrow().clone();
                if let (Some(cb), Some(e)) = (cb, entry(i)) {
                    cb(e);
                }
            };
            match e.keyCode() {
                123..=126 if !cmd => {
                    if n == 0 {
                        return true;
                    }
                    let next = match (cur, e.keyCode()) {
                        (None, _) => 0,
                        (Some(i), 123) => i.saturating_sub(1),
                        (Some(i), 124) => (i + 1).min(n - 1),
                        (Some(i), 125) => (i + COLS).min(n - 1),
                        (Some(i), _) => i.saturating_sub(COLS),
                    };
                    self.ivars().hover.set(Some(next));
                    self.scroll_to(next);
                    self.setNeedsDisplay(true);
                    true
                }
                36 | 76 => {
                    if !self.merge_if_ready() {
                        fire(&self.ivars().on_open, cur);
                    }
                    true
                }
                49 if !cmd => {
                    fire(&self.ivars().on_pin, cur);
                    true
                }
                8 if cmd => {
                    fire(&self.ivars().on_copy, cur);
                    true
                }
                51 | 117 => {
                    fire(&self.ivars().on_trash, cur);
                    true
                }
                _ => false,
            }
        }

        /// Scroll so cell `i` is fully visible.
        fn scroll_to(&self, i: usize) {
            let c = self.cell(i);
            let top = HEAD_H + 4.0;
            let bottom = self.bounds().size.height - 4.0;
            let mut y = self.ivars().scroll.get();
            if c.y < top {
                y -= top - c.y;
            } else if c.y + c.height > bottom {
                y += c.y + c.height - bottom;
            }
            self.ivars().scroll.set(y.max(0.0));
        }

        /// Highlight shot `i` (the socket's `recents:select:N`).
        pub fn set_highlight(&self, i: Option<usize>) {
            let n = self.count();
            let i = i.filter(|i| *i < n);
            self.ivars().hover.set(i);
            if let Some(i) = i {
                self.scroll_to(i);
            }
            self.setNeedsDisplay(true);
        }

        /// What Return does on the highlighted shot (`recents:open-selected`).
        pub fn open_highlighted(&self) -> bool {
            let entry = self.ivars().hover.get().and_then(|i| self.ivars().entries.borrow().get(i).cloned());
            let cb = self.ivars().on_open.borrow().clone();
            match (cb, entry) {
                (Some(cb), Some(e)) => {
                    cb(e);
                    true
                }
                _ => false,
            }
        }

        pub fn path_at(&self, i: usize) -> Option<String> {
            self.ivars().entries.borrow().get(i).map(|e| e.path.clone())
        }

        /// The highlighted shot (hover or arrow keys), for state / tests.
        pub fn highlighted(&self) -> Option<usize> {
            self.ivars().hover.get()
        }

        pub fn count(&self) -> usize {
            self.ivars().entries.borrow().len()
        }

        /// `ShotRecentView.remove(_:)` (after menu ▸ Delete).
        pub fn remove(&self, path: &str) {
            self.ivars().picked.borrow_mut().retain(|p| p != path);
            self.ivars().entries.borrow_mut().retain(|e| e.path != path);
            self.ivars().thumbs.borrow_mut().remove(path);
            self.setNeedsDisplay(true);
        }


        fn cell(&self, i: usize) -> Rect {
            let (c, r) = ((i % COLS) as f64, (i / COLS) as f64);
            Rect::new(PAD + c * (CELL_W + GAP), HEAD_H + PAD + r * (CELL_H + GAP) - self.ivars().scroll.get(), CELL_W, CELL_H)
        }

        fn index_at(&self, x: f64, y: f64) -> Option<usize> {
            if y <= HEAD_H {
                return None;
            }
            (0..self.ivars().entries.borrow().len()).find(|&i| self.cell(i).contains(x, y))
        }

        fn draw(&self) {
            let b = self.bounds();
            let white = |a: f64| Rgba::new(1.0, 1.0, 1.0, a);
            let all = Rect::new(0.5, 0.5, b.size.width - 1.0, b.size.height - 1.0);
            style::fill_rounded(all, 14.0, Rgba::new(0.10, 0.10, 0.10, 0.96));
            style::stroke_rounded(all, 14.0, 1.0, white(0.12));
            style::draw_text("Recent screenshots", PAD, 12.0, &style::system_font(13.0, Weight::Semibold), white(1.0));
            {
                let n = self.ivars().picked.borrow().len();
                let hints = self.ivars().hints.borrow();
                let (text, alpha) = if n == 0 {
                    (hints.idle.clone(), 0.45)
                } else {
                    (hints.picked.replace("{}", &n.to_string()), 0.9)
                };
                if !text.is_empty() {
                    let color = if n == 0 { white(alpha) } else { self.ivars().ui.get() };
                    style::draw_text(&text, PAD + 150.0, 13.5, &style::system_font(11.5, Weight::Medium), color);
                }
            }
            {
                // Vertical | Horizontal: how a Shift-click merge stacks.
                let hints = self.ivars().hints.borrow();
                let on = self.ivars().horizontal.get();
                let (a, b) = (self.pill_rect(false), self.pill_rect(true));
                let all = Rect::new(a.x, a.y, a.width + b.width, a.height);
                style::fill_rounded(all, 6.0, white(0.08));
                let lit = if on { b } else { a };
                style::fill_rounded(lit, 6.0, self.ivars().ui.get());
                let font = style::system_font(11.5, Weight::Medium);
                style::draw_text_centered(&hints.vertical, a, &font, white(if on { 0.6 } else { 1.0 }));
                style::draw_text_centered(&hints.horizontal, b, &font, white(if on { 1.0 } else { 0.6 }));
            }
            let entries = self.ivars().entries.borrow();
            if entries.is_empty() {
                style::draw_text(
                    "No screenshots yet — copy, save or pin a capture and it shows up here.",
                    PAD,
                    HEAD_H + 16.0,
                    &style::system_font(12.0, Weight::Regular),
                    white(0.55),
                );
                return;
            }
            let clip = NSRect::new(NSPoint::new(0.0, HEAD_H), NSSize::new(b.size.width, b.size.height - HEAD_H));
            objc2_app_kit::NSGraphicsContext::saveGraphicsState_class();
            objc2_app_kit::NSBezierPath::clipRect(clip);
            let thumbs = self.ivars().thumbs.borrow();
            let now = super::unix_now();
            let age_font = style::system_font(10.5, Weight::Regular);
            for (i, e) in entries.iter().enumerate() {
                let c = self.cell(i);
                if c.y + c.height < HEAD_H || c.y > b.size.height {
                    continue;
                }
                let well = Rect::new(c.x, c.y, c.width, c.height - 20.0);
                style::fill_rounded(well, 8.0, white(0.05));
                if let Some(t) = thumbs.get(&e.path) {
                    let s = t.size();
                    if s.width > 0.0 && s.height > 0.0 {
                        let k = ((well.width - 8.0) / s.width).min((well.height - 8.0) / s.height);
                        let (tw, th) = (s.width * k, s.height * k);
                        let r = Rect::new(well.mid_x() - tw / 2.0, well.mid_y() - th / 2.0, tw, th);
                        style::draw_image(t, r, 1.0);
                    }
                }
                let pick = self.ivars().picked.borrow().iter().position(|p| p == &e.path);
                if let Some(n) = pick {
                    // Picked for merging: a thick ring and its order badge.
                    let ui = self.ivars().ui.get();
                    style::stroke_rounded(well, 8.0, 3.0, ui);
                    let badge = Rect::new(well.x + 6.0, well.y + 6.0, 24.0, 24.0);
                    style::fill_rounded(badge, 12.0, ui);
                    style::draw_text_centered(
                        &(n + 1).to_string(),
                        badge,
                        &style::system_font(12.5, Weight::Bold),
                        white(1.0),
                    );
                } else if self.ivars().hover.get() == Some(i) {
                    style::stroke_rounded(well, 8.0, 2.0, self.ivars().ui.get());
                }
                style::draw_text(&age(e.at, now), c.x + 2.0, well.y + well.height + 4.0, &age_font, white(0.6));
            }
            objc2_app_kit::NSGraphicsContext::restoreGraphicsState_class();
            let _ = NSColor::clearColor();
        }
    }

    pub struct PanelIvars {
        pub grid: Retained<ShotRecentView>,
        pub on_escape: RefCell<Option<Rc<dyn Fn()>>>,
    }

    define_class!(
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShotRecentPanel"]
        #[ivars = PanelIvars]
        pub struct ShotRecentPanel;

        impl ShotRecentPanel {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool { true }

            #[unsafe(method(cancelOperation:))]
            fn cancel_operation(&self, _sender: Option<&AnyObject>) {
                self.escape();
            }

            #[unsafe(method(keyDown:))]
            fn key_down(&self, e: &NSEvent) {
                if e.keyCode() == 53 {
                    self.escape();
                    return;
                }
                self.ivars().grid.key(e);
            }

            /// Letting go of Shift (and Cmd) with 2+ picked merges them.
            #[unsafe(method(flagsChanged:))]
            fn flags_changed(&self, e: &NSEvent) {
                if !e.modifierFlags().intersects(PICK_MODS) {
                    self.ivars().grid.merge_if_ready();
                }
            }
        }

        unsafe impl NSObjectProtocol for ShotRecentPanel {}
    );

    impl ShotRecentPanel {
        pub fn create(mtm: MainThreadMarker, entries: Vec<ShotRecentEntry>, ui: Rgba) -> Retained<Self> {
            let grid = ShotRecentView::create(mtm, entries, ui);
            let size = grid.frame().size;
            let this = Self::alloc(mtm).set_ivars(PanelIvars { grid: grid.clone(), on_escape: RefCell::new(None) });
            let panel: Retained<Self> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), size),
                    styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            let base: &NSView = &grid;
            panel.setContentView(Some(base));
            panel.setFloatingPanel(true);
            panel.setLevel(objc2_app_kit::NSScreenSaverWindowLevel + 1);
            panel.setHidesOnDeactivate(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setOpaque(false);
            panel.setHasShadow(true);
            unsafe { panel.setReleasedWhenClosed(false) };
            panel
        }

        pub fn grid(&self) -> Retained<ShotRecentView> {
            self.ivars().grid.clone()
        }

        fn escape(&self) {
            // Esc peels the merge picks first, then closes the grid.
            if self.ivars().grid.has_picks() {
                self.ivars().grid.clear_picks();
                return;
            }
            let cb = self.ivars().on_escape.borrow().clone();
            if let Some(cb) = cb {
                cb();
            }
        }
    }
}

/// The open recent-screenshots panel (`recentPanel`). `Send` is asserted
/// like [`PinWindows`]: it is only touched on the main thread.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct RecentPanelSlot(Option<objc2::rc::Retained<recent_grid::ShotRecentPanel>>);

#[cfg(target_os = "macos")]
unsafe impl Send for RecentPanelSlot {}

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

/// The live controller, for the overlay view's mouse events (a click can
/// finish the session or ask for the recents grid, exactly like a key).
#[cfg(target_os = "macos")]
static LIVE_CONTROLLER: std::sync::Mutex<Option<std::sync::Weak<Mutex<ScreenshotController>>>> =
    std::sync::Mutex::new(None);

/// After the overlay view fed a mouse event into the session: complete a
/// finished session / open the recents grid on the next main-queue turn
/// (never inside the event dispatch itself).
#[cfg(target_os = "macos")]
fn pump_after_mouse() {
    crate::platform::run_on_main(|| {
        let Some(ctrl) = LIVE_CONTROLLER.lock().ok().and_then(|g| g.as_ref().and_then(|w| w.upgrade())) else {
            return;
        };
        let Ok(mut c) = ctrl.try_lock() else { return };
        if c.session.as_ref().is_some_and(|s| s.finished) {
            c.complete_finished_session();
            return;
        }
        if c.session.as_ref().is_some_and(|s| s.recent_requested) {
            if let Some(s) = c.session.as_mut() {
                s.recent_requested = false;
            }
            c.toggle_recent();
        }
        c.redraw_overlays();
    });
}

#[cfg(target_os = "macos")]
use crate::ui::view_base::as_any;

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
    // The recents grid panel takes key while it is open, so its keys never
    // reach the overlay target below. Tab / Shift+Tab there wrap the pill
    // stops (close the grid and move on to Copy Text / Screenshot); the
    // panel's own `keyDown:` handles only Esc.
    if c.recent_panel.0.is_some()
        && as_any(&*window)
            .downcast_ref::<recent_grid::ShotRecentPanel>()
            .is_some()
    {
        let k = key_input_from_event(event);
        if k.key_code == KEY_TAB && !k.cmd && !k.ctrl && !k.opt {
            if let Some(s) = c.session.as_mut() {
                s.cycle_stop(k.shift);
            }
            if c.session.as_ref().is_some_and(|s| s.recent_requested) {
                if let Some(s) = c.session.as_mut() {
                    s.recent_requested = false;
                }
                c.toggle_recent();
            }
            c.redraw_overlays();
            return std::ptr::null_mut();
        }
    }
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
            // ⌘R (`pressed(.recent)` → `onRecent`): the recent-screenshots grid.
            if c.session.as_ref().is_some_and(|s| s.recent_requested) {
                if let Some(s) = c.session.as_mut() {
                    s.recent_requested = false;
                }
                c.toggle_recent();
                // Keep the pill's stop highlight in step with the grid.
                c.redraw_overlays();
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
                PinKeyAction::Copy => {
                    if let Some(img) = c.pin_image(i) {
                        c.copy(&img, &ScreenshotConfig::load(), None);
                    }
                }
                PinKeyAction::Ignored => {}
            }
            // `PinPanel.handleKey` always returns true (Swift swallows the key).
            c.drop_key_monitor_if_idle();
            std::ptr::null_mut()
        }
        None => raw,
    }
}

/// A history PNG as a [`ShotImage`] (`ShotHistory.thumbnail(path, 100_000)`).
#[cfg(target_os = "macos")]
fn recent_image(path: &str) -> Option<ShotImage> {
    let png = std::fs::read(path).ok()?;
    let cg = pin_window::decode_png(&png)?;
    let (w, h) = (
        objc2_core_graphics::CGImage::width(Some(&cg)) as i64,
        objc2_core_graphics::CGImage::height(Some(&cg)) as i64,
    );
    Some(ShotImage::with_png(w, h, png))
}

#[cfg(target_os = "macos")]
fn reveal_in_workspace(path: &str) {
    // Uniform "reveal": the app's own file browser, never macOS Finder.
    crate::app::hooks::reveal_in_files(&[path.to_string()]);
}

/// The frame of the screen under the mouse (`mouseScreen`).
#[cfg(target_os = "macos")]
fn mouse_screen_frame() -> Option<objc2_foundation::NSRect> {
    let m = objc2_app_kit::NSEvent::mouseLocation();
    crate::platform::screen_containing(m.x, m.y)
        .or_else(crate::platform::main_screen)
        .map(|s| {
            let f = s.frame;
            objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(f.x, f.y),
                objc2_foundation::NSSize::new(f.w, f.h),
            )
        })
}

/// `PinPanel.menu()`: the pin window's right-click menu. The controller is
/// locked only inside each action, never while the menu is up.
#[cfg(target_os = "macos")]
fn pin_menu(ctrl: &std::sync::Weak<Mutex<ScreenshotController>>, window_number: i64) {
    use crate::ui::menu_kit::{popup, MenuEntry};
    let act = |f: fn(&mut ScreenshotController, usize)| {
        let ctrl = ctrl.clone();
        move || {
            let Some(c) = ctrl.upgrade() else { return };
            let Ok(mut c) = c.lock() else { return };
            if let Some(i) = c.pins.iter().position(|p| p.window_number == window_number) {
                f(&mut c, i);
            }
        }
    };
    let entries = vec![
        MenuEntry::item("Copy to clipboard", act(|c, i| {
            if let Some(img) = c.pin_image(i) {
                c.copy(&img, &ScreenshotConfig::load(), None);
            }
        })),
        MenuEntry::item("Save to file", act(|c, i| {
            if let Some(img) = c.pin_image(i) {
                c.save(&img, &ScreenshotConfig::load(), None, None);
            }
        })),
        MenuEntry::Separator,
        MenuEntry::item("Rotate Right", act(|c, i| c.rotate_pin(i, 1))),
        MenuEntry::item("Rotate Left", act(|c, i| c.rotate_pin(i, -1))),
        MenuEntry::item("Increase Opacity", act(|c, i| c.pin_opacity(i, 0.1))),
        MenuEntry::item("Decrease Opacity", act(|c, i| c.pin_opacity(i, -0.1))),
        MenuEntry::Separator,
        MenuEntry::Item {
            title: "\u{2318}-drag to drop it into another app".to_string(),
            checked: false,
            enabled: false,
            swatch: None,
            hover: None,
            action: Box::new(|| {}),
        },
        MenuEntry::item("Close", act(|c, i| c.close_pin(i))),
    ];
    popup(entries, None, objc2_app_kit::NSEvent::mouseLocation().x, objc2_app_kit::NSEvent::mouseLocation().y);
}

/// Run `f` on the live controller for the pin with `window_number` (the
/// `PinView` callbacks' shared lock-and-dispatch).
#[cfg(target_os = "macos")]
fn pin_with(
    ctrl: &std::sync::Weak<Mutex<ScreenshotController>>,
    window_number: i64,
    f: impl FnOnce(&mut ScreenshotController, usize),
) {
    let Some(c) = ctrl.upgrade() else { return };
    let Ok(mut c) = c.lock() else { return };
    if let Some(i) = c.pins.iter().position(|p| p.window_number == window_number) {
        f(&mut c, i);
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
    /// The overlay is up over the live screen and its frozen capture is still
    /// in flight (the async Hyper+X path). A finish waits for it.
    capture_pending: bool,
    /// Bumped per async capture; a late capture for an older session is dropped.
    capture_gen: u64,
    /// The merged-screenshot editor's origin: the session it replaced and the
    /// picked paths. Esc in the merge editor restores both (Recents reopens
    /// with the picks); any other finish drops them.
    merge_parent: Option<(ShotSession, Vec<String>, bool)>,
    /// A `-d` / `[screenshot] delay` timer is outstanding (Swift's `capturing`
    /// stays true across the `asyncAfter`, so a delayed capture is not idle).
    delay_pending: bool,
    /// Bumped when a delayed capture is scheduled or aborted so stale timers drop.
    delay_gen: u64,
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
    #[cfg(target_os = "macos")]
    recent_panel: RecentPanelSlot,
    /// A live screen recording (V): the `screencapture -v` child and the
    /// .mov it writes. The next Hyper+X stops it.
    recording: Option<(std::process::Child, String)>,
}

impl Default for ScreenshotController {
    fn default() -> Self {
        ScreenshotController::new()
    }
}

impl ScreenshotController {
    pub fn new() -> Self {
        #[cfg(not(test))]
        let home = std::env::var("HOME").unwrap_or_default();
        // Unit tests record fake PNGs: keep them out of the real cache dir.
        #[cfg(test)]
        let home = {
            let d = std::env::temp_dir().join(format!("ks-test-home-{}", std::process::id()));
            d.to_string_lossy().into_owned()
        };
        ScreenshotController {
            config: ScreenshotConfig::default(),
            state_path: format!("{home}/.cache/kitchen-sink/screenshot-state.json"),
            history: ShotHistory::new(format!("{home}/.cache/kitchen-sink/screenshots")),
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
            capture_pending: false,
            capture_gen: 0,
            merge_parent: None,
            delay_pending: false,
            delay_gen: 0,
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
            #[cfg(target_os = "macos")]
            recent_panel: RecentPanelSlot::default(),
            recording: None,
        }
    }

    /// Whether a screen recording is running (Hyper+X then stops it).
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Start recording `rect` (global, top-left points) with
    /// `screencapture -v` into the save folder.
    fn start_recording(&mut self, rect: Rect, cfg: &ScreenshotConfig) {
        let dir = expand_tilde(&cfg.save_path);
        let _ = std::fs::create_dir_all(&dir);
        let target = ShotFiles::target(&dir, &cfg.filename_pattern, "mov");
        let r = format!("-R{},{},{},{}", rect.x.round(), rect.y.round(), rect.w.round(), rect.h.round());
        match std::process::Command::new("/usr/sbin/screencapture")
            .args(["-v", "-x", "-k", &r, &target])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.note(format!("screenshot: recording {r} -> {target}"));
                self.recording = Some((child, target));
                // A red frame round the area and a pill to stop it, the whole
                // time (left out of the recording itself).
                #[cfg(target_os = "macos")]
                {
                    let weak = self.self_ref.clone();
                    rec_badge::show(rect, Box::new(move || {
                        if let Some(c) = weak.upgrade() {
                            if let Ok(mut c) = c.lock() {
                                c.stop_recording();
                            }
                        }
                    }));
                }
                #[cfg(not(target_os = "macos"))]
                ScreenToast::show(&mut self.toast, "Recording \u{2014} press Hyper+X to stop", "record.circle");
            }
            Err(e) => {
                self.note(format!("screenshot: recording failed: {e}"));
                ScreenToast::show(&mut self.toast, format!("Couldn't start recording: {e}"), "exclamationmark.triangle.fill");
            }
        }
    }

    /// Stop the recording (SIGINT, as Ctrl+C in Terminal), wait for the
    /// .mov to be finalised, then copy its path and add it to Recent Files.
    pub fn stop_recording(&mut self) -> Option<String> {
        let (mut child, target) = self.recording.take()?;
        #[cfg(target_os = "macos")]
        rec_badge::hide();
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as i32, libc::SIGINT);
        }
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() < std::time::Duration::from_secs(8) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        if std::path::Path::new(&target).exists() {
            self.note(format!("screenshot: recording saved {target}"));
            // Not under `cargo test`: the clipboard and the Recent shelf are the user's.
            #[cfg(not(test))]
            {
                crate::platform::set_text(&target);
                #[cfg(target_os = "macos")]
                crate::app::hooks::screenshot_saved_to_path_shelf(&target);
            }
            let shown = abbreviate_tilde(&target);
            ScreenToast::show(&mut self.toast, format!("Saved recording {shown} \u{b7} path copied"), "checkmark.circle.fill");
            self.last_output = json!({"outcome": "record", "path": target});
            Some(target)
        } else {
            ScreenToast::show(&mut self.toast, "The recording wasn't saved (Screen Recording permission?)", "exclamationmark.triangle.fill");
            None
        }
    }

    pub fn set_config(&mut self, cfg: ScreenshotConfig) {
        // Unit tests keep their temp history dir (see `new`).
        #[cfg(not(test))]
        if !cfg.history_dir.trim().is_empty() {
            self.history = ShotHistory::new(expand_tilde(cfg.history_dir.trim()));
        }
        self.config = cfg;
    }

    /// The only correct way to build a live controller: wrap it in its
    /// `Arc<Mutex>` AND wire the weak self-reference. Without the self-ref the
    /// overlay's key monitor never installs and its click/close/wheel/pinch
    /// callbacks hold dead weak refs — a full-screen overlay that ignores all
    /// input (the quick agent shipped exactly that and froze the screen).
    pub fn shared(cfg: ScreenshotConfig) -> Arc<Mutex<ScreenshotController>> {
        let mut c = ScreenshotController::new();
        c.set_config(cfg);
        let arc = Arc::new(Mutex::new(c));
        #[cfg(target_os = "macos")]
        arc.lock().unwrap().set_self_ref(Arc::downgrade(&arc));
        arc
    }

    /// True once [`shared`](Self::shared) / [`set_self_ref`] wired the
    /// controller; an overlay must never open without it.
    #[cfg(target_os = "macos")]
    pub fn is_wired(&self) -> bool {
        self.self_ref.strong_count() > 0
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
        if let Ok(mut g) = LIVE_CONTROLLER.lock() {
            *g = Some(weak.clone());
        }
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
        self.capture_pending = false;
        self.capture_gen = self.capture_gen.wrapping_add(1);
        self.delay_pending = false;
        self.delay_gen = self.delay_gen.wrapping_add(1);
        #[cfg(target_os = "macos")]
        {
            self.close_recent();
            self.dismiss_overlay();
        }
        self.session = None;
        #[cfg(target_os = "macos")]
        self.drop_key_monitor_if_idle();
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    /// `ScreenshotController.prewarm()`: mark the controller warm and hand the
    /// caller the `ShotOCR.warmUp` inputs the first time.
    ///
    /// The warm-up itself (a synchronous Vision recognition, seconds on a cold
    /// VM) deliberately runs *outside* this mutex: holding it across the OCR
    /// made `state` / `do:` replies — served on the single socket thread, which
    /// reads this controller — wait for the whole recognition (old bug #21).
    /// `None` when it already ran or off main (the AppKit bitmap context and
    /// Vision are main-thread only; `deliver_text` warm-starts lazily off main).
    pub fn prewarm_plan(&mut self) -> Option<crate::engines::screenshot_text::ShotOCRConfig> {
        self.prewarmed = true;
        if self.ocr_warm {
            return None;
        }
        self.ocr_warm = true;
        let cfg = crate::engines::screenshot_text::ShotOCRConfig {
            languages: self.config.ocr.languages.clone(),
            correction: self.config.ocr.correction,
            accurate: self.config.ocr.accurate,
            auto_accurate: self.config.ocr.auto_accurate,
        };
        // Planned on main (once per process); the warm-up itself runs on a
        // background thread (`ShotOCR::warm_up_in_background`).
        #[cfg(target_os = "macos")]
        {
            if objc2::MainThreadMarker::new().is_some() {
                return Some(cfg);
            }
            None
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
    pub fn redraw_overlays(&self) {
        for panel in &self.overlay_panels.0 {
            panel.redraw();
        }
    }

    /// Reload each overlay panel's layer contents from the session's base
    /// images (after a merge reorder replaced the canvas), then redraw.
    #[cfg(target_os = "macos")]
    fn reload_overlay_bases(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.base_dirty = false;
        }
        for (i, panel) in self.overlay_panels.0.iter().enumerate() {
            let img = self.session.as_ref().and_then(|s| s.base_images.get(i).cloned().flatten());
            panel.set_base(img.as_deref());
        }
        self.redraw_overlays();
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
        // `checkPermission`: open Privacy ▸ Screen Recording so the toast's
        // promise is real.
        #[cfg(target_os = "macos")]
        {
            crate::platform::open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            );
        }
        self.note("screenshot: no Screen Recording permission");
        false
    }

    pub fn trigger(&mut self, args: &ShotArgs) -> Result<(), String> {
        // Hyper+X while recording stops the recording instead of capturing.
        if self.recording.is_some() {
            self.stop_recording();
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            // The screen list is read fresh per capture (displays come and go).
            let screens = current_screens();
            if !screens.is_empty() {
                self.screens = screens;
            }
            if screen_capture_permitted() {
                return self.trigger_inner(args, &RealCapture, true);
            }
        }
        self.trigger_inner(args, &UnavailableCapture, true)
    }

    /// `trigger_with` (the public, test-facing entry): a thin wrapper over
    /// [`Self::trigger_inner`] that honours the `-d` / `[screenshot] delay`.
    pub fn trigger_with(
        &mut self,
        args: &ShotArgs,
        capture: &dyn ScreenCapture,
    ) -> Result<(), String> {
        self.trigger_inner(args, capture, true)
    }

    /// The shared capture body. `allow_delay` is false when re-entered from the
    /// delay timer, so a delayed trigger cannot schedule a second delay.
    fn trigger_inner(
        &mut self,
        args: &ShotArgs,
        capture: &dyn ScreenCapture,
        allow_delay: bool,
    ) -> Result<(), String> {
        let cfg = self.config.clone();
        if !cfg.enabled {
            self.note("screenshot: [screenshot] enabled = false");
            return Ok(());
        }
        if self.session.is_some() || self.capturing || self.delay_pending {
            return Ok(());
        }
        if !self.check_permission(&cfg) {
            return Ok(());
        }
        // Swift: `delay = args.delayMs > 0 ? args.delayMs : cfg.delayMs`, then
        // `asyncAfter`. A live controller schedules on the main queue and
        // returns; a bare controller (unit tests) has no weak self to upgrade
        // and captures straight away.
        let delay_ms = if args.delay_ms > 0 {
            args.delay_ms
        } else {
            cfg.delay_ms
        };
        #[cfg(target_os = "macos")]
        if allow_delay && delay_ms > 0 {
            let weak = self.self_ref.clone();
            if weak.strong_count() > 0 {
                self.delay_pending = true;
                self.delay_gen = self.delay_gen.wrapping_add(1);
                let dgen = self.delay_gen;
                let a = args.clone();
                dispatch2::DispatchQueue::main()
                    .after(
                        dispatch2::DispatchTime::NOW.time(delay_ms * 1_000_000),
                        move || {
                            if let Some(c) = weak.upgrade() {
                                if let Ok(mut c) = c.try_lock() {
                                    if !c.delay_pending || c.delay_gen != dgen {
                                        return;
                                    }
                                    c.delay_pending = false;
                                    let _ = c.trigger_inner(&a, &RealCapture, false);
                                }
                            }
                        },
                    )
                    .ok();
                return Ok(());
            }
        }

        // The interactive overlay goes up first, over the live screen, and the
        // capture runs behind it (the overlay panels are left out of capture).
        #[cfg(target_os = "macos")]
        if capture.live()
            && matches!(args.mode, ShotMode::Gui | ShotMode::Text)
            && self.self_ref.strong_count() > 0
            && objc2::MainThreadMarker::new().is_some()
        {
            self.start_async_session(args, &cfg);
            return Ok(());
        }

        // KR1 (PRD §7.3.4): time the capture and the path to the overlay.
        let t0 = std::time::Instant::now();
        self.capturing = true;
        // No separate `shareable_content()` round-trip here: `capture` gets its
        // display list itself (from a cache). A second SCShareableContent fetch
        // cost 15–20 ms on every Hyper+X press.
        let cap_t0 = std::time::Instant::now();
        let res = capture.capture(&self.screens);
        let capture_ms = cap_t0.elapsed().as_millis() as i64;
        let path_to_overlay_ms = t0.elapsed().as_millis() as i64;
        self.capturing = false;
        match res {
            Ok(images) if !images.is_empty() => {
                match args.mode {
                    ShotMode::Gui | ShotMode::Text => {
                        let mut s = self.new_overlay_session(args, &cfg);
                        #[cfg(target_os = "macos")]
                        set_session_images(&mut s, &images);
                        self.session = Some(s);
                        #[cfg(target_os = "macos")]
                        self.present_overlay();
                        let line = format!(
                            "screenshot: capture {} ms, {} ms to overlay (overlay built in {} ms)",
                            capture_ms,
                            path_to_overlay_ms,
                            t0.elapsed().as_millis() as i64 - path_to_overlay_ms
                        );
                        crate::app::logging::ws_log(&line);
                        self.note(line);
                    }
                    ShotMode::Full | ShotMode::Screen => {
                        self.direct(args, &cfg, &images);
                        self.note(format!(
                            "screenshot {}: capture {} ms",
                            args.mode.as_str(),
                            capture_ms
                        ));
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
                self.reply = Some(Vec::new());
                Ok(())
            }
        }
    }

    /// A fresh interactive session over the current screens (no frozen
    /// images yet): displays, the pointer, and any `--region` preselect.
    fn new_overlay_session(&self, args: &ShotArgs, cfg: &ScreenshotConfig) -> ShotSession {
        let mut s = ShotSession::new(cfg.clone(), args.clone(), self.state_path.clone());
        for scr in &self.screens {
            let mut disp = ShotDisplay::new(scr.id, scr.frame, scr.scale);
            disp.top_inset = scr.top_inset;
            s.add_display(disp);
        }
        // Start with the pointer where it is, so the mode pill
        // (and loupe) show before the first mouse move.
        #[cfg(target_os = "macos")]
        {
            let m = objc2_app_kit::NSEvent::mouseLocation();
            let hit = s.displays.iter().position(|d| {
                m.x >= d.frame.x && m.x < d.frame.x + d.frame.w
                    && m.y >= d.frame.y && m.y < d.frame.y + d.frame.h
            });
            if let Some(i) = hit {
                let f = s.displays[i].frame;
                s.mouse = Some((i, Point::new(m.x - f.x, f.y + f.h - m.y)));
            }
        }
        // `--region` / `--last-region`: preselect before the
        // panel presents so the ring/selection show on open.
        if let Some((rect, idx)) = initial_region(args, &s) {
            s.test_select(rect, Some(idx));
        }
        s
    }

    /// Hyper+X fast path: present the overlay at once over the live screen,
    /// capture on a background thread, and drop the frozen image in when it
    /// lands ([`Self::land_capture`], ~30–60 ms later). The screen is assumed
    /// not to change in that window, so the swap is invisible; the overlay
    /// panels are `NSWindowSharingType::None`, so the capture never contains
    /// them. A finish before the image lands waits for it.
    #[cfg(target_os = "macos")]
    fn start_async_session(&mut self, args: &ShotArgs, cfg: &ScreenshotConfig) {
        let t0 = std::time::Instant::now();
        let s = self.new_overlay_session(args, cfg);
        self.session = Some(s);
        self.capture_pending = true;
        self.capture_gen = self.capture_gen.wrapping_add(1);
        let gen = self.capture_gen;
        self.present_overlay();
        let shown_ms = t0.elapsed().as_millis();
        let screens = self.screens.clone();
        let weak = self.self_ref.clone();
        let spawned = std::thread::Builder::new().name("ks-shot-capture".into()).spawn(move || {
            let cap_t0 = std::time::Instant::now();
            let res = sck::capture_displays(&screens);
            let capture_ms = cap_t0.elapsed().as_millis();
            let res = SendCapture(res);
            crate::platform::run_on_main(move || {
                let res = res;
                let Some(ctrl) = weak.upgrade() else { return };
                let Ok(mut c) = ctrl.lock() else { return };
                c.land_capture(gen, res.0, shown_ms, capture_ms);
            });
        });
        if spawned.is_err() {
            self.land_capture(gen, Err(CaptureError::new("capture thread")), shown_ms, 0);
        }
    }

    /// Main thread: the async capture is back. Fill the frozen images (model +
    /// panel layers), then complete a finish that was waiting on it; a failed
    /// capture closes the overlay with the fail toast.
    #[cfg(target_os = "macos")]
    fn land_capture(
        &mut self,
        gen: u64,
        res: Result<Vec<CapturedImage>, CaptureError>,
        shown_ms: u128,
        capture_ms: u128,
    ) {
        if gen != self.capture_gen || !self.capture_pending {
            return;
        }
        self.capture_pending = false;
        if self.session.is_none() {
            return; // closed (Esc) before the image landed
        }
        match res {
            Ok(images) if !images.is_empty() => {
                if let Some(s) = self.session.as_mut() {
                    set_session_images(s, &images);
                }
                for (i, panel) in self.overlay_panels.0.iter().enumerate() {
                    let img = self
                        .session
                        .as_ref()
                        .and_then(|s| s.base_images.get(i).cloned().flatten());
                    panel.set_base(img.as_deref());
                }
                self.redraw_overlays();
                let line = format!(
                    "screenshot: overlay shown in {shown_ms} ms, capture landed {capture_ms} ms later"
                );
                crate::app::logging::ws_log(&line);
                self.note(line);
                self.complete_finished_session();
            }
            _ => {
                let cfg = self.config.clone();
                ScreenToast::show(
                    &mut self.toast,
                    cfg.fail_toast.clone(),
                    "exclamationmark.triangle.fill",
                );
                self.note("screenshot: capture failed");
                crate::app::logging::ws_log("screenshot: async capture failed");
                self.end_session(ShotOutcome::Abort, None, None);
                self.reply = Some(Vec::new());
            }
        }
    }

    /// Port of Swift `ScreenshotController.direct` + `stitch` (~754–793): the
    /// non-interactive `full` / `screen -n` delivery (no overlay). `screen`
    /// picks one display's capture; `full` stitches every display.
    fn direct(&mut self, args: &ShotArgs, cfg: &ScreenshotConfig, images: &[CapturedImage]) {
        #[cfg(target_os = "macos")]
        {
            let screen: Option<ScreenRef> = if args.mode == ShotMode::Screen {
                args.screen_number
                    .and_then(|n| usize::try_from(n).ok())
                    .and_then(|n| self.screens.get(n).copied())
                    .or_else(|| self.screen_under_mouse())
            } else {
                self.screen_under_mouse()
            };
            let img = match args.mode {
                ShotMode::Screen => screen
                    .and_then(|s| images.iter().find(|i| i.display == s.id))
                    .and_then(|i| i.image.as_ref())
                    .and_then(|img| encode_shot_image(img)),
                _ => shot_cg::stitch(&self.screens, images),
            };
            let Some(img) = img else {
                self.reply = Some(Vec::new());
                return;
            };
            let (w, h) = (img.width, img.height);
            self.last_output = json!({"outcome": args.mode.as_str(), "size": [w, h]});
            if let Some(png) = &img.png {
                self.history.record_png(png, cfg.history.max(0) as usize);
            }
            if let Some(p) = args.path.as_deref() {
                self.save(&img, cfg, screen, Some(p));
            }
            if args.clipboard || (args.path.is_none() && !args.raw && !args.pin) {
                self.copy(&img, cfg, screen);
            }
            if args.pin {
                if let Some(s) = screen {
                    let scale = s.scale.max(1.0);
                    let frame = Rect::new(
                        s.frame.x,
                        s.frame.y,
                        w as f64 / scale,
                        h as f64 / scale,
                    );
                    self.pin(&img, Some(frame), screen);
                }
            }
            // Swift `direct`: `reply?(a.raw ? png(img) : Data())` — no
            // `--print-geometry` geometry on the direct path (matches `finish`'s
            // overlay path only).
            self.reply = if args.raw {
                img.png.clone()
            } else {
                Some(Vec::new())
            };
        }
    }

    /// `NSScreen` under the pointer as a [`ScreenRef`], or the first screen.
    #[cfg(target_os = "macos")]
    fn screen_under_mouse(&self) -> Option<ScreenRef> {
        let m = objc2_app_kit::NSEvent::mouseLocation();
        self.screens
            .iter()
            .find(|s| {
                m.x >= s.frame.x
                    && m.x < s.frame.x + s.frame.w
                    && m.y >= s.frame.y
                    && m.y < s.frame.y + s.frame.h
            })
            .or_else(|| self.screens.first())
            .copied()
    }

    /// No overlay up and no capture in flight (a session has finished).
    pub fn is_idle(&self) -> bool {
        self.session.is_none()
            && !self.capturing
            && !self.capture_pending
            && !self.delay_pending
    }

    /// Whether the recent-screenshots grid panel is open.
    #[cfg(target_os = "macos")]
    pub fn has_recent_open(&self) -> bool {
        self.recent_panel.0.is_some()
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
        // Swift `onSaved` → `PathShelf.add([path], why: .screenshot)`.
        crate::app::hooks::screenshot_saved_to_path_shelf(&target);
        // Swift `write`'s `copyPathAfterSave`: put the saved path on the
        // clipboard after a successful save.
        if cfg.copy_path_after_save {
            #[cfg(target_os = "macos")]
            {
                let _ = write_text_pasteboard(&target);
            }
        }
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
                w.set_png(png.to_vec());
                panel.window_number = w.window_number();
                panel.key = true;
                let ctrl = self.self_ref.clone();
                let wn = panel.window_number;
                w.set_on_menu(std::rc::Rc::new(move || pin_menu(&ctrl, wn)));
                // `PinView.mouseDown`/`scrollWheel`/`magnify` all reach back
                // through the controller (weak), so the panel never owns it.
                let close = self.self_ref.clone();
                w.set_on_close(std::rc::Rc::new(move || {
                    pin_with(&close, wn, |c, i| c.close_pin(i));
                }));
                let wheel = self.self_ref.clone();
                w.set_on_wheel(std::rc::Rc::new(move |f: f64| {
                    pin_with(&wheel, wn, |c, i| c.pin_zoom(i, f));
                }));
                let pinch = self.self_ref.clone();
                w.set_on_magnify(std::rc::Rc::new(move |f: f64| {
                    pin_with(&pinch, wn, |c, i| c.pin_zoom(i, f));
                }));
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

    /// `showRecentScreenshots(_:)`: toggle the recent-screenshots grid over
    /// the screen under the mouse (⌘R in the overlay).
    #[cfg(target_os = "macos")]
    pub fn toggle_recent(&mut self) {
        use objc2::DefinedClass;
        use objc2::Message;
        use std::rc::Rc;
        if self.close_recent() {
            return;
        }
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        let cfg = ScreenshotConfig::load();
        let ui = crate::ui::theme::Rgba::new(cfg.ui_color.r, cfg.ui_color.g, cfg.ui_color.b, 1.0);
        let panel = recent_grid::ShotRecentPanel::create(mtm, self.history.entries(), ui);
        let grid = panel.grid();
        let me = self.self_ref.clone();
        let with = move |f: Box<dyn FnOnce(&mut ScreenshotController)>| {
            if let Some(c) = me.upgrade() {
                if let Ok(mut c) = c.lock() {
                    f(&mut c);
                }
            }
        };
        let w = with.clone();
        *panel.ivars().on_escape.borrow_mut() = Some(Rc::new(move || {
            w(Box::new(|c| {
                c.close_recent();
                c.redraw_overlays();
            }))
        }));
        // Click / Return: edit it (Esc in the editor comes back here).
        let w = with.clone();
        let horizontal = cfg.merge.horizontal;
        *grid.ivars().on_open.borrow_mut() = Some(Rc::new(move |e: ShotRecentEntry| {
            w(Box::new(move |c| c.open_merged_in_editor(std::slice::from_ref(&e.path), horizontal)))
        }));
        let w = with.clone();
        *grid.ivars().on_pin.borrow_mut() = Some(Rc::new(move |e: ShotRecentEntry| {
            w(Box::new(move |c| c.reopen_recent_as_pin(&e.path)))
        }));
        let w = with.clone();
        *grid.ivars().on_copy.borrow_mut() = Some(Rc::new(move |e: ShotRecentEntry| {
            w(Box::new(move |c| c.copy_recent(&e.path)))
        }));
        let weak_for_trash = objc2::rc::Weak::from_retained(&grid);
        *grid.ivars().on_trash.borrow_mut() = Some(Rc::new(move |e: ShotRecentEntry| {
            trash_recent(&e.path, &weak_for_trash);
        }));
        let w = with.clone();
        *grid.ivars().on_merge.borrow_mut() = Some(Rc::new(move |picks: Vec<ShotRecentEntry>, horizontal: bool| {
            let paths: Vec<String> = picks.into_iter().map(|e| e.path).collect();
            w(Box::new(move |c| c.open_merged_in_editor(&paths, horizontal)))
        }));
        grid.set_hints(recent_grid::MergeHints {
            idle: cfg.merge.hint.clone(),
            picked: cfg.merge.picked_hint.clone(),
            vertical: cfg.merge.vertical_label.clone(),
            horizontal: cfg.merge.horizontal_label.clone(),
        });
        grid.set_horizontal(cfg.merge.horizontal);
        let w = with.clone();
        // "Open in Popup" returns the keyboard to the overlay under the grid.
        let popup_parent: Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> =
            self.overlay_panels.0.first().map(|p| {
                let w: &objc2_app_kit::NSWindow = p;
                w.retain()
            });
        let weak_grid = objc2::rc::Weak::from_retained(&grid);
        *grid.ivars().on_menu.borrow_mut() = Some(Rc::new(move |e: ShotRecentEntry| {
            use crate::ui::menu_kit::{popup_context, MenuEntry};
            let (w1, w2, w3, w4, w5, w6) = (w.clone(), w.clone(), w.clone(), w.clone(), w.clone(), w.clone());
            let (p1, p2, p3, p4, p5, p6, p7) = (
                e.path.clone(), e.path.clone(), e.path.clone(), e.path.clone(), e.path.clone(), e.path.clone(),
                e.path.clone(),
            );
            let g = weak_grid.clone();
            let parent = popup_parent.clone();
            let Some(view) = weak_grid.load() else { return };
            popup_context(
                vec![
                    MenuEntry::item("Edit in Screenshot Editor", move || {
                        w1(Box::new(move |c| c.open_merged_in_editor(std::slice::from_ref(&p1), horizontal)))
                    }),
                    MenuEntry::item("Pin on Screen", move || w5(Box::new(move |c| c.reopen_recent_as_pin(&p6)))),
                    MenuEntry::item("Copy", move || w2(Box::new(move |c| c.copy_recent(&p2)))),
                    MenuEntry::item("Copy Text", move || {
                        w6(Box::new(move |c| {
                            c.close_recent();
                            c.redraw_overlays();
                            c.copy_text_from_files(std::slice::from_ref(&p7));
                        }))
                    }),
                    MenuEntry::item("Open in Popup", move || {
                        w3(Box::new(move |c| {
                            // The grid goes away; the overlay stays live under
                            // the popup and takes the keyboard back on close.
                            c.close_recent();
                            c.redraw_overlays();
                            let parent_ns = parent.as_ref().map(|w| &**w);
                            crate::views::file_popup::show(&p3, parent_ns);
                        }));
                    }),
                    MenuEntry::item("Reveal in Files", move || {
                        w4(Box::new(|c| c.abort_session()));
                        reveal_in_workspace(&p4);
                    }),
                    MenuEntry::Separator,
                    MenuEntry::item("Move to Trash", move || trash_recent(&p5, &g)),
                ],
                &view,
            );
        }));
        if let Some(f) = mouse_screen_frame() {
            let sz = panel.frame().size;
            panel.setFrameOrigin(objc2_foundation::NSPoint::new(
                (f.origin.x + f.size.width / 2.0 - sz.width / 2.0).round(),
                (f.origin.y + f.size.height / 2.0 - sz.height / 2.0).round(),
            ));
        }
        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        self.recent_panel.0 = Some(panel);
        // `recent_open` is the pill's third stop (`ShotSession::stop`) and what
        // Tab / Shift+Tab cycles from; keep it in step with the live panel.
        if let Some(s) = self.session.as_mut() {
            s.recent_open = true;
        }
    }

    /// Close the recent grid; `true` when one was open.
    #[cfg(target_os = "macos")]
    pub fn close_recent(&mut self) -> bool {
        if let Some(s) = self.session.as_mut() {
            s.recent_open = false;
        }
        match self.recent_panel.0.take() {
            Some(p) => {
                p.orderOut(None);
                true
            }
            None => false,
        }
    }

    /// Leave the merge editor for the session it replaced, with the Recents
    /// grid open again and the same screenshots picked.
    #[cfg(target_os = "macos")]
    fn return_from_merge(&mut self) {
        let Some((mut parent, mut picks, mut horizontal)) = self.merge_parent.take() else { return };
        // The strip may have reordered / restacked: come back that way.
        if let Some(m) = self.session.as_ref().and_then(|s| s.merge.as_ref()) {
            picks = m.paths.clone();
            horizontal = m.horizontal;
        }
        self.dismiss_overlay();
        parent.recent_open = false;
        parent.recent_requested = false;
        self.session = Some(parent);
        self.present_overlay();
        self.toggle_recent();
        if let Some(panel) = self.recent_panel.0.as_ref() {
            panel.grid().set_picks(picks);
            panel.grid().set_horizontal(horizontal);
        }
        self.redraw_overlays();
    }

    /// `finish(s, .abort)`: drop the overlay session (and the grid), plus any
    /// in-flight async capture or pending retake delay so nothing pops back up.
    pub fn abort_session(&mut self) {
        self.merge_parent = None;
        self.capture_pending = false;
        self.capture_gen = self.capture_gen.wrapping_add(1);
        self.delay_pending = false;
        self.delay_gen = self.delay_gen.wrapping_add(1);
        #[cfg(target_os = "macos")]
        {
            self.close_recent();
            crate::views::file_popup::close();
        }
        if let Some(s) = self.session.as_mut() {
            if !s.finished {
                s.finish(ShotOutcome::Abort);
            }
        }
        self.complete_finished_session();
        if self.session.is_some() {
            self.close();
        }
    }

    /// Recents ▸ Shift-click 2+ screenshots: stack them top to bottom (pick
    /// order, numbered 1, 2, 3 … in a left gutter) and open the result in
    /// the screenshot editor with the stack preselected, ready to annotate,
    /// copy, save or pin. The editor canvas uses enough pixels per point
    /// (up to `merge-max-scale`) to keep the screenshots at native
    /// resolution; a stack too tall for that is shrunk to fit the screen.
    #[cfg(target_os = "macos")]
    pub fn open_merged_in_editor(&mut self, paths: &[String], horizontal: bool) {
        // Any picture format, upright (EXIF orientation applied); a file
        // that is not really an image is skipped, never drawn as garbage.
        let (readable, images): (Vec<String>, Vec<objc2::rc::Retained<objc2_core_graphics::CGImage>>) = paths
            .iter()
            .filter_map(|p| shot_cg::decode_image_file(p, EDITOR_MAX_IMAGE_PX).map(|i| (p.clone(), i)))
            .unzip();
        if images.is_empty() {
            self.note("screenshot: no readable picture to open");
            crate::ui::toast::show("Not a readable picture", "exclamationmark.triangle");
            return;
        }
        if images.len() < paths.len() {
            crate::ui::toast::show(
                &format!("{} of {} could not be read as pictures", paths.len() - images.len(), paths.len()),
                "exclamationmark.triangle",
            );
        }
        // One picture: just that picture to edit (no number gutter, no frame).
        let single = images.len() == 1;
        let screens = current_screens();
        if !screens.is_empty() {
            self.screens = screens;
        }
        let Some(scr) = self.screen_under_mouse() else { return };
        self.close_recent();
        // Keep the capture the merge was started from (Esc comes back to it
        // with the Recents grid), instead of aborting it.
        self.dismiss_overlay();
        let parent = self.session.take().filter(|p| !p.finished);
        let cfg = ScreenshotConfig::load();
        self.config = cfg.clone();
        let m = &cfg.merge;
        // Gutter / gap / label are points on a `px` display: the stack is
        // built in screenshot pixels.
        let px = scr.scale.max(1.0);
        let sizes: Vec<(f64, f64)> = images
            .iter()
            .map(|i| {
                (
                    objc2_core_graphics::CGImage::width(Some(i)) as f64,
                    objc2_core_graphics::CGImage::height(Some(i)) as f64,
                )
            })
            .collect();
        let gutter = if single { 0.0 } else { m.gutter * px };
        let layout = merge_layout(&sizes, horizontal, gutter, m.gap * px, m.gap * px);
        // 2+ photos: keep the left edge free for the reorder strip.
        let stage = merge_stage_left(
            layout.width,
            layout.height,
            scr.frame.w,
            scr.frame.h,
            scr.top_inset,
            if single { 0.0 } else { MergeStrip::reserve() },
            48.0,
            scr.scale,
            m.max_scale,
        );
        let (cw, ch) = (
            (scr.frame.w * stage.scale).round() as i64,
            (scr.frame.h * stage.scale).round() as i64,
        );
        let Some(canvas) = shot_cg::merge_canvas(
            &images,
            cw,
            ch,
            &stage,
            &layout,
            m.background,
            m.border_color,
            if single { 0.0 } else { m.border_width * px },
        ) else {
            self.note("screenshot: merge canvas failed");
            return;
        };
        let mut args = ShotArgs::default();
        args.mode = ShotMode::Gui;
        let mut s = ShotSession::new(cfg.clone(), args, self.state_path.clone());
        let mut disp = ShotDisplay::new(scr.id, scr.frame, scr.scale);
        disp.top_inset = scr.top_inset;
        s.add_display(disp);
        s.set_base_image(0, Some(canvas));
        s.text_mode = false;
        let ms = objc2_app_kit::NSEvent::mouseLocation();
        s.mouse = Some((0, Point::new(ms.x - scr.frame.x, scr.frame.y + scr.frame.h - ms.y)));
        s.test_select(stage.rect, Some(0));
        // The numbers: real annotations, so Layers / Delete can remove them.
        if !single {
            s.doc.objects = merge_labels(&stage, &layout, m, px, cfg.counter_outline);
            s.merge = Some(MergeEdit {
                paths: readable,
                images: images.clone(),
                horizontal,
                px,
                layout: layout.clone(),
                stage: stage.clone(),
                canvas: (cw, ch),
                display: 0,
                drag: None,
            });
        }
        self.session = Some(s);
        self.merge_parent = parent.map(|p| (p, paths.to_vec(), horizontal));
        self.capture_pending = false;
        self.capture_gen = self.capture_gen.wrapping_add(1);
        self.present_overlay();
        self.redraw_overlays();
        let line = format!(
            "screenshot: opened {} picture(s) in the editor ({}x{} px, {:.0}% of native)",
            images.len(),
            (layout.width * stage.fit).round(),
            (layout.height * stage.fit).round(),
            stage.fit * 100.0
        );
        crate::app::logging::ws_log(&line);
        self.note(line);
    }

    /// Files / Find Recent Files ▸ right-click ▸ "Open in Screenshot Editor"
    /// (one picture) or "Merge N Pictures in Screenshot Editor": drop any
    /// live capture, then open the pictures in the editor, stacked like a
    /// Recents merge (`[screenshot] merge-*`). Esc just closes the editor.
    #[cfg(target_os = "macos")]
    pub fn open_images_in_editor(&mut self, paths: &[String]) {
        if !self.is_idle() {
            self.abort_session();
        }
        let horizontal = ScreenshotConfig::load().merge.horizontal;
        self.open_merged_in_editor(paths, horizontal);
    }

    /// Copy Text on pictures already on disk (Recents ▸ Copy Text, Files ▸
    /// Copy Text from Picture): each is read like a Hyper+X Copy Text (QR
    /// payloads first, else OCR), the texts joined by a blank line, then the
    /// usual card. Main thread.
    #[cfg(target_os = "macos")]
    pub fn copy_text_from_files(&mut self, paths: &[String]) {
        let cfg = self.session.as_ref().map(|s| s.cfg.clone()).unwrap_or_else(ScreenshotConfig::load);
        let mut parts: Vec<String> = Vec::new();
        let mut codes = 0;
        for p in paths {
            let Some(cg) = shot_cg::decode_image_file(p, EDITOR_MAX_IMAGE_PX) else { continue };
            let Some(img) = encode_shot_image(&cg) else { continue };
            let read = ocr_text(&img, &cfg.ocr);
            codes += read.codes;
            if !read.text.is_empty() {
                parts.push(read.text);
            }
        }
        let text = parts.join("\n\n");
        self.last_output = json!({"outcome": "text", "files": paths.len(), "codes": codes, "chars": text.chars().count()});
        if text.is_empty() {
            ScreenToast::show(&mut self.toast, cfg.no_text_toast.clone(), "exclamationmark.triangle.fill");
        } else if codes > 0 && parts.len() == codes {
            let what = if codes == 1 { "a QR code".to_string() } else { format!("{codes} codes") };
            self.copy_text_titled(&text, &cfg, Some(cfg.code_toast.replace("{}", &what)));
        } else {
            self.copy_text(&text, &cfg);
        }
        self.note(format!("screenshot: text from {} file(s), {} chars", paths.len(), text.chars().count()));
    }

    /// Recents ▸ Copy (menu or Cmd+C): the shot onto the clipboard. The
    /// capture under the grid stays as it was (annotations and all).
    #[cfg(target_os = "macos")]
    pub fn copy_recent(&mut self, path: &str) {
        let Some(img) = recent_image(path) else { return };
        self.close_recent();
        self.redraw_overlays();
        let cfg = self.session.as_ref().map(|s| s.cfg.clone()).unwrap_or_else(ScreenshotConfig::load);
        self.copy(&img, &cfg, None);
    }

    /// The grid's `onOpen`: abort the overlay, then pin that file centred on
    /// the screen under the mouse.
    #[cfg(target_os = "macos")]
    pub fn reopen_recent_as_pin(&mut self, path: &str) {
        let Some(img) = recent_image(path) else { return };
        self.abort_session();
        let Some(_mtm) = objc2::MainThreadMarker::new() else { return };
        self.pin_centered(&img);
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.note(format!("screenshot: reopened {name}"));
    }

    /// The pinned image at `i` (from its window), for copy / save.
    pub fn pin_image(&self, i: usize) -> Option<ShotImage> {
        #[cfg(target_os = "macos")]
        {
            let w = self.pin_windows.0.get(i)?.as_ref()?;
            let png = w.png();
            let cg = pin_window::decode_png(&png)?;
            let (pw, ph) = (
                objc2_core_graphics::CGImage::width(Some(&cg)) as i64,
                objc2_core_graphics::CGImage::height(Some(&cg)) as i64,
            );
            return Some(ShotImage::with_png(pw, ph, png));
        }
        #[allow(unreachable_code)]
        None
    }

    /// `PinPanel.rotate(_:)` (menu ▸ Rotate Right / Left).
    pub fn rotate_pin(&mut self, i: usize, quarters: i64) {
        if let Some(p) = self.pins.get_mut(i) {
            p.rotate(quarters);
        }
        #[cfg(target_os = "macos")]
        if let Some(Some(w)) = self.pin_windows.0.get(i) {
            if let Some((_, _, png)) = shot_cg::rotate_png(&w.png(), quarters) {
                w.show_rotated(png, quarters % 2 != 0);
            }
        }
    }

    /// `PinPanel.opacity(_:)` (menu ▸ Increase / Decrease Opacity).
    pub fn pin_opacity(&mut self, i: usize, delta: f64) {
        let Some(p) = self.pins.get_mut(i) else { return };
        p.opacity(delta);
        #[cfg(target_os = "macos")]
        if let Some(Some(w)) = self.pin_windows.0.get(i) {
            w.set_alpha(p.alpha);
        }
    }

    /// `PinPanel.setZoom`: scale a live pin by `factor` around its centre and
    /// apply the model frame to the real window.
    pub fn pin_zoom(&mut self, i: usize, factor: f64) {
        let Some(p) = self.pins.get_mut(i) else { return };
        p.set_zoom(p.zoom * factor, None);
        #[cfg(target_os = "macos")]
        if let Some(Some(w)) = self.pin_windows.0.get(i) {
            w.apply_frame(self.pins[i].frame);
        }
    }

    /// `PinPanel.closePin` (menu ▸ Close).
    pub fn close_pin(&mut self, i: usize) {
        #[cfg(target_os = "macos")]
        if let Some(Some(w)) = self.pin_windows.0.get(i) {
            w.close_pin();
        }
        if i < self.pins.len() {
            self.pins.remove(i);
            #[cfg(target_os = "macos")]
            if i < self.pin_windows.0.len() {
                self.pin_windows.0.remove(i);
            }
        }
        #[cfg(target_os = "macos")]
        self.drop_key_monitor_if_idle();
    }

    pub fn copy_text(&mut self, text: &str, cfg: &ScreenshotConfig) {
        self.copy_text_titled(text, cfg, None);
    }

    /// [`Self::copy_text`] with the toast's title given (`None` = the
    /// `text-toast` line count).
    pub fn copy_text_titled(&mut self, text: &str, cfg: &ScreenshotConfig, title: Option<String>) {
        let _ = crate::platform::set_text(text);
        self.last_output["copied"] = json!(true);
        let summary = crate::engines::screenshot_text::ShotOCR::summary(text);
        let title = title.unwrap_or_else(|| cfg.text_toast.replace("{}", &summary));
        if !title.is_empty() {
            ScreenToast::show(&mut self.toast, title, "text.viewfinder");
        }
    }

    pub fn deliver_text(&mut self, img: &ShotImage, cfg: &ScreenshotConfig, args: &ShotArgs) {
        // Real Vision pass over the rendered capture (Swift `deliverText` →
        // `ShotOCR.text(img, cfg.ocr)`). Failures (no permission, off-main, a
        // non-decodable image) degrade to the empty string → the no-text toast.
        let started = now_millis();
        let read = ocr_text(img, &cfg.ocr);
        let text = read.text;
        let ms = (now_millis() - started).max(0);
        let chars = text.chars().count();
        self.last_output = json!({
            "outcome": "text",
            "size": [img.width, img.height],
            "chars": chars,
            "codes": read.codes,
            "accurate": crate::engines::screenshot_text::ShotOCR::use_accurate(cfg.ocr.accurate, cfg.ocr.auto_accurate),
            "text": crate::engines::screenshot_text::ShotOCR::truncate(&text, 4000),
            "ms": ms,
        });
        if text.is_empty() {
            ScreenToast::show(
                &mut self.toast,
                cfg.no_text_toast.clone(),
                "exclamationmark.triangle.fill",
            );
        } else if read.codes > 0 {
            let what = if read.codes == 1 { "a QR code".to_string() } else { format!("{} codes", read.codes) };
            self.copy_text_titled(&text, cfg, Some(cfg.code_toast.replace("{}", &what)));
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
        // The frozen image is still in flight: `land_capture` completes it.
        if self.capture_pending && outcome != ShotOutcome::Abort {
            return;
        }
        // Esc in the merge editor: back to the capture + Recents it came from.
        #[cfg(target_os = "macos")]
        if outcome == ShotOutcome::Abort && self.merge_parent.is_some() {
            self.return_from_merge();
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
        self.merge_parent = None;
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
        let record_rect = self
            .session
            .as_ref()
            .filter(|s| s.record_requested)
            .and_then(|s| s.global_selection());
        // Every capture that was not cancelled (and a retake / recording)
        // remembers its area for L and `--last-region`.
        let retake = self.session.as_ref().is_some_and(|s| s.retake_requested);
        let keep_region = outcome != ShotOutcome::Abort
            || self.session.as_ref().is_some_and(|s| s.retake_requested || s.record_requested);
        if keep_region {
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
        self.capture_pending = false;
        self.capture_gen = self.capture_gen.wrapping_add(1);
        #[cfg(target_os = "macos")]
        self.dismiss_overlay();
        self.session = None;
        #[cfg(target_os = "macos")]
        self.drop_key_monitor_if_idle();
        if let Some(r) = record_rect {
            self.reply = None;
            self.start_recording(r, &cfg);
            return;
        }
        if retake {
            self.reply = None;
            self.last_output = json!({"outcome": "retake", "delayMs": cfg.retake_delay_ms});
            let secs = (cfg.retake_delay_ms as f64 / 1000.0).round() as i64;
            ScreenToast::show(&mut self.toast, format!("Retaking in {secs} s\u{2026}"), "timer");
            let args = ShotArgs {
                mode: ShotMode::Gui,
                delay_ms: cfg.retake_delay_ms,
                last_region: true,
                ..ShotArgs::default()
            };
            let _ = self.trigger(&args);
            return;
        }
        if outcome == ShotOutcome::Abort || img.is_none() {
            self.last_output = json!({"outcome": "abort"});
            self.note("screenshot: aborted");
            self.reply = None;
            return;
        }
        let img = img.unwrap();
        if outcome == ShotOutcome::Text {
            // Copy Text captures go to Recents too (the picture you read).
            if let Some(png) = &img.png {
                self.history.record_png(png, cfg.history.max(0) as usize);
            }
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
                // The dismissable preview popup (Esc / Cmd+W closes it).
                if cfg.preview {
                    self.pin(&img, global_frame, screen);
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
            // The Recents grid for agents: `recents` / `recents:open` opens
            // it (overlay up), `recents:close`, `recents:select:N`,
            // `recents:open-selected` (= Return: the editor), and
            // `recents:copy-text:N` (Copy Text on shot N).
            #[cfg(target_os = "macos")]
            "recents" => {
                let (sub, rest) = arg.split_once(':').unwrap_or((arg, ""));
                match sub {
                    "" | "open" => {
                        if self.session.is_none() {
                            return Err("recents: no overlay (do:screenshot:show first)".to_string());
                        }
                        if self.recent_panel.0.is_none() {
                            self.toggle_recent();
                        }
                        Ok(())
                    }
                    "close" => {
                        self.close_recent();
                        Ok(())
                    }
                    "select" => {
                        let Some(p) = self.recent_panel.0.as_ref() else { return Err("recents not open".to_string()) };
                        let i: usize = rest.parse().map_err(|_| "recents:select:N".to_string())?;
                        if i >= p.grid().count() {
                            return Err(format!("recents: no shot {i}"));
                        }
                        p.grid().set_highlight(Some(i));
                        Ok(())
                    }
                    "open-selected" => {
                        let Some(p) = self.recent_panel.0.as_ref().cloned() else { return Err("recents not open".to_string()) };
                        // `on_open` locks this controller: run it after the
                        // socket hook releases the lock.
                        let Some(mtm) = objc2::MainThreadMarker::new() else { return Err("main thread only".to_string()) };
                        let p = dispatch2::MainThreadBound::new(p, mtm);
                        dispatch2::DispatchQueue::main().exec_async(move || {
                            if let Some(mtm) = objc2::MainThreadMarker::new() {
                                p.get(mtm).grid().open_highlighted();
                            }
                        });
                        Ok(())
                    }
                    "copy-text" => {
                        let Some(p) = self.recent_panel.0.as_ref() else { return Err("recents not open".to_string()) };
                        let i: usize = rest.parse().map_err(|_| "recents:copy-text:N".to_string())?;
                        let Some(path) = p.grid().path_at(i) else { return Err(format!("recents: no shot {i}")) };
                        self.close_recent();
                        self.copy_text_from_files(&[path]);
                        Ok(())
                    }
                    _ => Err(format!("unknown recents verb {sub}")),
                }
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
            // `panel-click:X,Y` — a click on the side panel / Tool Settings tab
            // (coordinates from `sidePanelLayout` / `toolTab` in the state).
            "panel-click" => {
                let nums: Vec<f64> = arg.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                let [x, y] = nums[..] else {
                    return Err("panel-click:X,Y".to_string());
                };
                match self.session.as_mut().map(|s| s.panel_click(Point::new(x, y))) {
                    Some(true) => Ok(()),
                    Some(false) => Err("side panel / tab not shown".to_string()),
                    None => Err("no session".to_string()),
                }
            }
            // `merge-move:FROM,TO` — the reorder strip's drag (0-based slots).
            #[cfg(target_os = "macos")]
            "merge-move" => {
                let nums: Vec<usize> = arg.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                let [from, to] = nums[..] else {
                    return Err("merge-move:FROM,TO".to_string());
                };
                match self.session.as_mut().map(|s| s.move_merged(from, to)) {
                    Some(true) => {
                        #[cfg(target_os = "macos")]
                        if objc2::MainThreadMarker::new().is_some() {
                            self.reload_overlay_bases();
                        }
                        Ok(())
                    }
                    Some(false) => Err("no merged stack, or a bad slot".to_string()),
                    None => Err("no session".to_string()),
                }
            }
            // `merge-direction:vertical|horizontal|toggle` — the strip's toggle.
            #[cfg(target_os = "macos")]
            "merge-direction" => {
                let want = match arg {
                    "vertical" => Some(false),
                    "horizontal" => Some(true),
                    "toggle" => None,
                    _ => return Err("merge-direction:vertical|horizontal|toggle".to_string()),
                };
                if self.session.as_ref().and_then(|s| s.merge.as_ref()).is_none() {
                    return Err("no merged stack".to_string());
                }
                if self.session.as_mut().is_some_and(|s| s.set_merge_horizontal(want))
                    && objc2::MainThreadMarker::new().is_some()
                {
                    self.reload_overlay_bases();
                }
                Ok(())
            }
            _ => Err(
                "show[:MS] | show-text[:MS] | mode:text|screenshot | text | select:X,Y,W,H | \
                 tool:NAME | draw:X1,Y1,X2,Y2 | key:SPEC | copy | accept | save:PATH | save-ok \
                 | pin | unpin | close | side-panel | panel-click:X,Y | merge-move:FROM,TO \
                 | merge-direction:vertical|horizontal|toggle"
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
            "ocrAccurateReady": crate::engines::screenshot_text::ShotOCR::accurate_ready(),
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
        #[cfg(target_os = "macos")]
        {
            st["recents"] = match self.recent_panel.0.as_ref() {
                Some(p) => json!({
                    "shown": true,
                    "count": p.grid().count(),
                    "highlighted": p.grid().highlighted(),
                }),
                None => json!({"shown": false}),
            };
        }
        let Some(s) = &self.session else {
            return st;
        };
        st["displays"] = json!(s
            .displays
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let f = d.frame;
                #[cfg(target_os = "macos")]
                let visible = self
                    .overlay_panels
                    .0
                    .get(i)
                    .map(|p| p.isVisible())
                    .unwrap_or(false);
                json!({
                    "id": d.id as i64,
                    "frame": [f.x as i64, f.y as i64, f.w as i64, f.h as i64],
                    "scale": d.scale,
                    "key": d.key,
                    "wid": d.wid,
                    "visible": visible,
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
        st["finished"] = json!(s.finished);
        st["dragging"] = json!(s.is_dragging());
        st["modePill"] = match s.chrome_rects.get() {
            Some(c) => json!({"x": c.pill.x, "y": c.pill.y, "w": c.pill.w, "h": c.pill.h}),
            None => Value::Null,
        };
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
        #[cfg(target_os = "macos")]
        if let Some(m) = &s.merge {
            let r = |r: Rect| json!([r.x, r.y, r.w, r.h]);
            let strip = s.merge_strip(m.display);
            st["merge"] = json!({
                "paths": m.paths,
                "horizontal": m.horizontal,
                "strip": strip.as_ref().map(|l| r(l.strip)),
                "rows": strip.as_ref().map(|l| l.rows.iter().map(|x| r(*x)).collect::<Vec<_>>()),
                "direction": strip.as_ref().map(|l| l.direction.iter().map(|x| r(*x)).collect::<Vec<_>>()),
            });
        }
        st["toolSize"] = match s.panel_size() {
            Some((v, _, _)) => json!(v),
            None => Value::Null,
        };
        st["color"] = json!(s.current_color().hex());
        st["grid"] = json!({"on": s.state.grid, "size": s.state.grid_size});
        st["hexEdit"] = match &s.hex_edit {
            Some(h) => json!(h.text),
            None => Value::Null,
        };
        st["sidePanelLayout"] = match s.panel_display().and_then(|d| s.panel_layout(d)) {
            Some(l) => l.to_json(),
            None => Value::Null,
        };
        st["toolTab"] = match s.panel_display().and_then(|d| s.tool_tab_rect(d)) {
            Some(r) => json!([r.x, r.y, r.w, r.h]),
            None => Value::Null,
        };
        st["helpShown"] = json!(s.cfg.show_help && s.selection.is_none());
        st["editingText"] = json!(s.editing.is_some());
        st["wheel"] = json!(s.wheel.is_some());
        st["grabbing"] = json!(s.grabbing.is_some());
        st["pickedHex"] = json!(s.picked_hex);
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
/// What Copy Text read: barcode payloads when the area holds a QR / barcode
/// (and `text-barcodes` is on), else the OCR text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadText {
    pub text: String,
    /// How many barcodes the text came from (0 = OCR).
    pub codes: usize,
}

fn ocr_text(img: &ShotImage, ocr: &ShotOCRConfigModel) -> ReadText {
    #[cfg(target_os = "macos")]
    {
        if objc2::MainThreadMarker::new().is_none() {
            return ReadText::default();
        }
        let Some(png) = img.png.as_deref() else {
            return ReadText::default();
        };
        let Some(image) = pin_window::decode_png(png) else {
            return ReadText::default();
        };
        if ocr.barcodes {
            let codes = crate::engines::screenshot_text::ShotOCR::barcodes(&image);
            if !codes.is_empty() {
                return ReadText { text: codes.join("\n"), codes: codes.len() };
            }
        }
        let cfg = crate::engines::screenshot_text::ShotOCRConfig {
            languages: ocr.languages.clone(),
            correction: ocr.correction,
            accurate: crate::engines::screenshot_text::ShotOCR::use_accurate(ocr.accurate, ocr.auto_accurate),
            auto_accurate: false,
        };
        let lines = crate::engines::screenshot_text::ShotOCR::recognize_text(&image, &cfg);
        let text = crate::engines::screenshot_text::ShotOCR::tidy_paths(
            &crate::engines::screenshot_text::ShotOCR::join_columns(&lines, ocr.column_gap),
        );
        ReadText { text, codes: 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (img, ocr);
        ReadText::default()
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

/// Put a plain string on the general pasteboard (Swift `write`'s
/// `copyPathAfterSave`: `pb.setString(path, forType: .string)`). Main-thread
/// only, like [`write_image_pasteboard`]; returns `false` off the main thread.
#[cfg(target_os = "macos")]
fn write_text_pasteboard(text: &str) -> bool {
    // Keep the off-main guard: the seam itself is thread-agnostic.
    if objc2::MainThreadMarker::new().is_none() {
        return false;
    }
    crate::platform::set_text(text)
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

/// The main-thread half of the `screenshot[\tWORDS]` socket verb, shared by the
/// main daemon and the quick agent: start/handle the capture session, then
/// answer `reply` (a caller that wants output: `--raw`, `--print-geometry`, …)
/// right away when the session is already over, else once the interactive
/// overlay closes. `reply = None` closes straight away. Call on the main thread.
pub fn run_screenshot_message(
    controller: Arc<Mutex<ScreenshotController>>,
    words: Vec<String>,
    reply: Option<crate::app::socket::Reply>,
) {
    let (finished, data) = {
        let mut c = controller.lock().unwrap();
        c.reply = None;
        // Never open an overlay that cannot receive input (see `shared`).
        #[cfg(target_os = "macos")]
        if !c.is_wired() {
            crate::app::logging::ws_log("screenshot: refused — controller not wired (use ScreenshotController::shared)");
            drop(c);
            if let Some(reply) = reply {
                reply.send_bytes(b"");
            }
            return;
        }
        let _ = c.handle(&words);
        #[cfg(target_os = "macos")]
        c.redraw_overlays();
        crate::app::logging::log_hotkey_shown(Some("screenshot"));
        let finished = c.is_idle();
        (finished, if finished { c.reply.take() } else { None })
    };
    let Some(reply) = reply else { return };
    if finished {
        reply.send_bytes(data.as_deref().unwrap_or_default());
        return;
    }
    // An interactive overlay is up: answer when it closes.
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut c = controller.lock().unwrap();
        if c.is_idle() {
            let data = c.reply.take();
            drop(c);
            reply.send_bytes(data.as_deref().unwrap_or_default());
            return;
        }
    });
}

/// The recording frame's AppKit rect for `rect` (global top-left points, as
/// `screencapture -R` takes them): AppKit grows up from the *primary*
/// screen's bottom-left, so the flip uses the primary height `ph` on every
/// display (a screen left of / above the primary has negative x / y).
pub fn rec_frame_area(rect: Rect, ph: f64, pad: f64) -> Rect {
    Rect::new(rect.x - pad, ph - rect.y - rect.h - pad, rect.w + pad * 2.0, rect.h + pad * 2.0)
}

/// While `V` records: a red frame round the recorded area and a pill above
/// it ("Recording — click or Hyper+X to stop"). Both are left out of screen
/// captures, so they never end up in the video.
#[cfg(target_os = "macos")]
mod rec_badge {
    use super::Rect;
    use crate::ui::style::{self, Weight};
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSEvent, NSPanel, NSScreen, NSView, NSWindowCollectionBehavior,
        NSWindowSharingType, NSWindowStyleMask,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::RefCell;

    thread_local! {
        static LIVE: RefCell<Vec<Retained<NSPanel>>> = const { RefCell::new(Vec::new()) };
    }

    pub struct PillIvars {
        on_click: RefCell<Option<Box<dyn Fn()>>>,
        label: String,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSRecBadgePill"]
        #[ivars = PillIvars]
        pub struct Pill;

        unsafe impl NSObjectProtocol for Pill {}

        impl Pill {
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {}

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, _e: &NSEvent) {
                let Some(mtm) = MainThreadMarker::new() else { return };
                if let Some(f) = self.ivars().on_click.borrow_mut().take() {
                    // Next run-loop turn: the click stops the recording, which
                    // tears this panel down.
                    let f = dispatch2::MainThreadBound::new(f, mtm);
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        if let Some(mtm) = MainThreadMarker::new() {
                            (f.get(mtm))();
                        }
                    });
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _r: NSRect) {
                let b = self.bounds();
                let r = crate::ui::chrome::Rect::new(0.5, 0.5, b.size.width - 1.0, b.size.height - 1.0);
                style::fill_rounded(r, r.height / 2.0, crate::ui::theme::Rgba::new(0.12, 0.12, 0.12, 0.94));
                let dot = crate::ui::chrome::Rect::new(12.0, (b.size.height - 10.0) / 2.0, 10.0, 10.0);
                style::fill_rounded(dot, 5.0, crate::ui::theme::Rgba::new(1.0, 0.23, 0.19, 1.0));
                let f = style::system_font(12.5, Weight::Medium);
                style::draw_text_left(
                    &self.ivars().label,
                    crate::ui::chrome::Rect::new(30.0, (b.size.height - 16.0) / 2.0, b.size.width - 40.0, 16.0),
                    &f,
                    crate::ui::theme::Rgba::new(1.0, 1.0, 1.0, 1.0),
                );
            }
        }
    );

    fn panel(mtm: MainThreadMarker, frame: NSRect, mouse: bool) -> Retained<NSPanel> {
        let p = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        p.setLevel(1001);
        p.setOpaque(false);
        p.setBackgroundColor(Some(&NSColor::clearColor()));
        p.setHasShadow(false);
        p.setHidesOnDeactivate(false);
        p.setIgnoresMouseEvents(!mouse);
        p.setSharingType(NSWindowSharingType::None);
        unsafe { p.setReleasedWhenClosed(false) };
        p.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        p
    }

    /// `rect`: the recorded area, global points with a top-left origin (as
    /// `screencapture -R` takes it).
    pub fn show(rect: Rect, on_click: Box<dyn Fn()>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        hide();
        let screens = NSScreen::screens(mtm);
        let Some(primary) = screens.iter().next() else { return };
        let ph = primary.frame().size.height;
        // AppKit frames grow up from the primary screen's bottom-left.
        let f = super::rec_frame_area(rect, ph, 3.0);
        let area = NSRect::new(NSPoint::new(f.x, f.y), NSSize::new(f.w, f.h));
        let frame = panel(mtm, area, false);
        let fv = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::ZERO, area.size));
        fv.setWantsLayer(true);
        if let Some(l) = fv.layer() {
            l.setBorderWidth(2.0);
            l.setBorderColor(Some(&NSColor::systemRedColor().CGColor()));
            l.setCornerRadius(3.0);
        }
        frame.setContentView(Some(&fv));
        frame.orderFrontRegardless();

        let label = "Recording \u{2014} click or Hyper+X to stop".to_string();
        let (tw, _) = style::text_size(&label, &style::system_font(12.5, Weight::Medium));
        let (w, h) = (tw + 44.0, 28.0);
        // Above the area when there is room, else just inside its top edge.
        let mut y = area.origin.y + area.size.height + 8.0;
        if y + h > ph {
            y = area.origin.y + area.size.height - h - 8.0;
        }
        let at = NSRect::new(NSPoint::new(area.origin.x + (area.size.width - w) / 2.0, y), NSSize::new(w, h));
        let pill_panel = panel(mtm, at, true);
        let pill: Retained<Pill> = {
            let this = Pill::alloc(mtm).set_ivars(PillIvars { on_click: RefCell::new(Some(on_click)), label });
            unsafe { msg_send![super(this), initWithFrame: NSRect::new(NSPoint::ZERO, at.size)] }
        };
        let pv: &NSView = &pill;
        pill_panel.setContentView(Some(pv));
        pill_panel.orderFrontRegardless();
        LIVE.with(|l| *l.borrow_mut() = vec![frame, pill_panel]);
    }

    pub fn hide() {
        LIVE.with(|l| {
            for p in l.borrow_mut().drain(..) {
                p.orderOut(None);
            }
        });
    }
}

/// Recents ▸ Move to Trash (menu or Delete): to the Trash (recoverable),
/// never a plain delete; the cell goes from the grid.
#[cfg(target_os = "macos")]
fn trash_recent(path: &str, grid: &objc2::rc::Weak<recent_grid::ShotRecentView>) {
    let url = objc2_foundation::NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(path));
    let ok = objc2_foundation::NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .is_ok();
    if ok {
        if let Some(g) = grid.load() {
            g.remove(path);
        }
        crate::ui::toast::show("Moved to Trash", "trash");
    } else {
        crate::ui::toast::show("Couldn't move it to the Trash", "exclamationmark.triangle.fill");
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
        // The floating preview: the same pin a screenshot's preview uses
        // (Esc / double-click closes it, Cmd-drag drops it into an app).
        #[cfg(target_os = "macos")]
        if cfg.preview {
            self.pin_centered(&shot.image);
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
        // The reply names what happened: the saved path, else "copied", else
        // (preview only) "shown".
        path.unwrap_or_else(|| if copy { "copied" } else { "shown" }.to_string())
    }

    /// Pin `img` centred on the screen under the mouse: real size, but never
    /// more than [`PIN_MAX_SCREEN_FRACTION`] of the screen (a 200-line
    /// pane-shot is 8800 px tall; at real size its preview covered the
    /// whole screen and off it).
    #[cfg(target_os = "macos")]
    pub fn pin_centered(&mut self, img: &ShotImage) {
        let scale = mouse_screen_scale().max(1.0);
        let (iw, ih) = (img.width as f64 / scale, img.height as f64 / scale);
        let frame = mouse_screen_frame().map(|f| {
            let (w, h) = pin_fit(iw, ih, f.size.width, f.size.height);
            Rect::new(f.origin.x + f.size.width / 2.0 - w / 2.0, f.origin.y + f.size.height / 2.0 - h / 2.0, w, h)
        });
        self.pin(img, frame, None);
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
    let p = objc2_app_kit::NSEvent::mouseLocation();
    crate::platform::screen_containing(p.x, p.y)
        .or_else(crate::platform::main_screen)
        .map_or(2.0, |s| s.scale)
}

pub fn epoch_millis() -> i64 {
    now_millis()
}

/// The largest share of the screen a centred preview pin may take.
pub const PIN_MAX_SCREEN_FRACTION: f64 = 0.8;

/// `(w, h)` points for a pin of an image `iw` × `ih` points on a screen
/// `sw` × `sh`: real size when it fits, else scaled down (aspect kept) to
/// [`PIN_MAX_SCREEN_FRACTION`] of the screen.
pub fn pin_fit(iw: f64, ih: f64, sw: f64, sh: f64) -> (f64, f64) {
    let k = (sw * PIN_MAX_SCREEN_FRACTION / iw.max(1.0))
        .min(sh * PIN_MAX_SCREEN_FRACTION / ih.max(1.0))
        .min(1.0);
    ((iw * k).round(), (ih * k).round())
}

/// Palette gate for Pane Shot (both palettes): herdr is installed.
pub fn pane_shot_listed(sections: &HashMap<String, HashMap<String, String>>) -> bool {
    let cfg = crate::engines::pane_shot::PaneShotConfig::from_entries(&sections.get("pane-shot").cloned().unwrap_or_default());
    crate::engines::pane_shot::Herdr::available(&cfg)
}

/// `[pane-shot]` from a commands.toml text (pure scan; both processes).
pub fn pane_shot_config(text: &str) -> crate::engines::pane_shot::PaneShotConfig {
    let sections = crate::app::sections::parse_sections(text);
    crate::engines::pane_shot::PaneShotConfig::from_entries(&sections.get("pane-shot").cloned().unwrap_or_default())
}

/// `pane-shot[\tWORDS]` for both the main daemon and the quick agent (one
/// implementation, rule 7): render the terminal pane off the main thread,
/// then save / copy / preview through `on_main`, and answer `reply` with the
/// saved path, `copied`, or `error: …`. Main thread (it reads the screen
/// scale).
#[cfg(target_os = "macos")]
pub fn run_pane_shot_message(
    controller: Arc<Mutex<ScreenshotController>>,
    words: Vec<String>,
    cfg: crate::engines::pane_shot::PaneShotConfig,
    reply: Option<crate::app::socket::Reply>,
    on_main: Box<dyn FnOnce(Box<dyn FnOnce() + Send>) + Send>,
) {
    let args = match crate::engines::pane_shot::PaneShotArgs::parse(&words) {
        Ok(a) => a,
        Err(p) => {
            if let Some(r) = reply {
                r.send(&format!("error: {}", p.message));
            }
            return;
        }
    };
    let scale = if objc2::MainThreadMarker::new().is_some() { mouse_screen_scale() } else { 2.0 };
    std::thread::spawn(move || {
        let started = now_millis();
        let result = pane_shot_image(&args, &cfg, scale);
        on_main(Box::new(move || {
            let mut c = controller.lock().unwrap();
            let line = match result {
                Ok(shot) => c.deliver_pane_shot(&shot, &args, &cfg, started),
                Err(f) => {
                    c.pane_shot_failed(&f.message);
                    format!("error: {}", f.message)
                }
            };
            #[cfg(target_os = "macos")]
            c.install_key_monitor();
            drop(c);
            if let Some(r) = reply {
                r.send(&line);
            }
        }));
    });
}

/// `on_main` for [`run_pane_shot_message`] in a process without its own main
/// queue (the quick agent): GCD's main queue.
#[cfg(target_os = "macos")]
pub fn gcd_main(job: Box<dyn FnOnce() + Send>) {
    dispatch2::DispatchQueue::main().exec_async(job);
}

// __PART_G__

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the quick agent built its controller with a bare
    /// `Arc::new`, so the overlay had no key monitor and dead mouse callbacks
    /// (Hyper+X froze the screen). `shared` must always wire the self-ref.
    #[cfg(target_os = "macos")]
    #[test]
    fn shared_controller_is_wired_and_bare_one_is_not() {
        let shot = ScreenshotController::shared(ScreenshotConfig::default());
        assert!(shot.lock().unwrap().is_wired());
        let bare = Arc::new(Mutex::new(ScreenshotController::new()));
        assert!(!bare.lock().unwrap().is_wired());
    }

    fn ws_words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn hyper_x_while_recording_stops_the_recording() {
        let mut c = ScreenshotController::default();
        let dir = std::env::temp_dir().join(format!("ks-rec-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("rec.mov").to_string_lossy().into_owned();
        std::fs::write(&target, b"mov").unwrap();
        // A stand-in for `screencapture -v`: SIGINT ends it, as it does the real one.
        let child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        c.recording = Some((child, target.clone()));
        assert!(c.is_recording());
        assert!(c.trigger(&ShotArgs::default()).is_ok());
        assert!(!c.is_recording(), "the second Hyper+X stops, it does not capture");
        assert!(c.session.is_none());
        assert_eq!(c.last_output["path"], json!(target));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn w_key_picks_the_front_app_window_under_the_pointer() {
        let w = |x, y, wd, h, layer, pid| ScreenWindow { bounds: Rect::new(x, y, wd, h), layer, pid };
        let wins = [
            w(0.0, 0.0, 2000.0, 24.0, 25, 1),     // menu bar layer
            w(100.0, 100.0, 300.0, 200.0, 0, 42), // ours (skipped)
            w(50.0, 50.0, 800.0, 600.0, 0, 7),    // the app window behind
            w(0.0, 0.0, 10.0, 10.0, 0, 8),        // a sliver
        ];
        assert_eq!(window_under(&wins, Point::new(150.0, 150.0), 42), Some(Rect::new(50.0, 50.0, 800.0, 600.0)));
        assert_eq!(window_under(&wins, Point::new(150.0, 150.0), 1), Some(Rect::new(100.0, 100.0, 300.0, 200.0)));
        assert_eq!(window_under(&wins, Point::new(5.0, 5.0), 42), None, "menu bar / slivers never");
        assert_eq!(window_under(&wins, Point::new(1900.0, 900.0), 42), None);
    }

    #[test]
    fn window_shot_args_parse_defaults() {
        let a = WindowShotArgs::parse(&ws_words(&[])).unwrap();
        assert_eq!(a, WindowShotArgs::default());
        assert_eq!(a.scale, 2.0);
        assert_eq!(a.view, None);
        assert_eq!(a.out, None);
    }

    #[test]
    fn window_shot_args_parse_full() {
        let a = WindowShotArgs::parse(&ws_words(&[
            "--view", "files", "--out", "/tmp/x.png", "--scale", "1",
        ]))
        .unwrap();
        assert_eq!(a.view.as_deref(), Some("files"));
        assert_eq!(a.out.as_deref(), Some("/tmp/x.png"));
        assert_eq!(a.scale, 1.0);
    }

    #[test]
    fn window_shot_args_parse_short_flags() {
        let a = WindowShotArgs::parse(&ws_words(&["-v", "ai", "-o", "out.png", "-s", "2"])).unwrap();
        assert_eq!(a.view.as_deref(), Some("ai"));
        assert_eq!(a.out.as_deref(), Some("out.png"));
        assert_eq!(a.scale, 2.0);
    }

    #[test]
    fn window_shot_args_reject_bad_scale() {
        let e = WindowShotArgs::parse(&ws_words(&["--scale", "3"])).unwrap_err();
        assert!(e.0.contains("--scale is 1 or 2"), "{}", e.0);
    }

    #[test]
    fn window_shot_args_reject_missing_value() {
        let e = WindowShotArgs::parse(&ws_words(&["--view"])).unwrap_err();
        assert!(e.0.contains("--view needs a value"), "{}", e.0);
    }

    #[test]
    fn window_shot_args_reject_unknown_flag() {
        let e = WindowShotArgs::parse(&ws_words(&["--bogus"])).unwrap_err();
        assert!(e.0.contains("unknown argument --bogus"), "{}", e.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recent_grid_size_and_age() {
        use super::recent_grid::{age, grid_size};
        // 4 columns of 172 + 3 gaps of 10 + 16 padding each side.
        assert_eq!(grid_size(0), (750.0, 110.0));
        assert_eq!(grid_size(3).1, 40.0 + 16.0 + 132.0 + 16.0);
        // Rows cap at 3.5 visible.
        assert_eq!(grid_size(40).1, 40.0 + 16.0 + 3.5 * 142.0 - 10.0 + 16.0);
        assert_eq!(age(100, 130), "just now");
        assert_eq!(age(0, 7200), "2 h ago");
        assert_eq!(age(0, 100_000), "yesterday");
    }
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
        // Unique per call: tests run in parallel, and a shared dir let one
        // test's persisted state leak into another (flaky `side_panel_grid_and_tab`).
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("ws-shot-{}-{name}-{n}", std::process::id()));
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
    fn snapshot_reflects_handled_events() {
        let mut s = session();
        let before = s.snapshot();
        // A button-ring tool tap selects the drawing tool…
        s.handle(ShotEvent::Pressed(ShotTool::Rectangle));
        let after = s.snapshot();
        assert_ne!(before, after, "handling an event changed the snapshot");
        assert_eq!(after.tool, Some(ShotTool::Rectangle));
        // …and reaching the recents stop raises the grid request.
        assert_eq!(after.stop, 1);
        s.handle(ShotEvent::GotoStop(2));
        assert!(s.snapshot().recent_requested, "the recents grid was requested");
    }

    #[test]
    fn config_defaults_and_parsing() {
        let d = ScreenshotConfig::default();
        assert!(d.enabled);
        assert_eq!(d.button_size, 34.0);
        assert_eq!(d.buttons.len(), 20, "default ring has no inert size badge");
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

    fn ki(code: u16, c: Option<char>, cmd: bool, ctrl: bool) -> KeyInput {
        KeyInput { key_code: code, chars: c, cmd, ctrl, shift: false, opt: false, esc_streak: 0 }
    }

    #[test]
    fn l_reselects_the_last_area() {
        let mut s = session();
        assert!(!s.handle_key(&ki(37, Some('l'), false, false)) || s.selection.is_none(), "no last area: nothing selected");
        s.state.last_region = Some(crate::engines::screenshot_annotations::ShotRegion {
            display: 1,
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 200.0,
        });
        assert!(s.handle_key(&ki(37, Some('l'), false, false)));
        let r = s.selection.expect("L selected the last area");
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 20.0, 300.0, 200.0));
        s.state.last_region.as_mut().unwrap().display = 99;
        s.selection = None;
        assert!(!s.select_last_region(), "a display that is gone: nothing");
    }

    #[test]
    fn cmd_t_asks_for_a_retake_of_the_selection() {
        let mut s = session();
        s.handle_key(&ki(17, Some('t'), true, false));
        assert!(!s.retake_requested && !s.finished, "no selection: Cmd+T does nothing");
        s.test_select(Rect::new(10.0, 10.0, 100.0, 80.0), Some(0));
        s.handle_key(&ki(17, Some('t'), true, false));
        assert!(s.retake_requested);
        assert_eq!(s.outcome, Some(ShotOutcome::Abort), "the overlay closes for the wait");
        // T alone still picks the Text tool.
        let mut s = session();
        s.handle_key(&ki(17, Some('t'), false, false));
        assert_eq!(s.tool, Some(ShotTool::Text));
    }

    #[test]
    fn r_is_the_outline_box_and_f_the_filled_one() {
        let mut s = session();
        s.handle_key(&ki(15, Some('r'), false, false));
        assert_eq!(s.tool, Some(ShotTool::Selection), "R = outline");
        s.handle_key(&ki(3, Some('f'), false, false));
        assert_eq!(s.tool, Some(ShotTool::Rectangle), "F = filled");
        assert_eq!(ShotTool::Selection.display_name(), "Rectangle");
        assert_eq!(ShotTool::Rectangle.display_name(), "Filled rectangle");
    }

    #[test]
    fn help_card_is_a_subset_of_the_full_shortcut_list() {
        let card = shortcut_rows(true, |_, d| d.to_string());
        let all = shortcut_rows(false, |_, d| d.to_string());
        assert!(card.len() < all.len());
        assert!(card.iter().all(|r| all.contains(r)));
        for key in ["V", "L", "\u{2318}P", "\u{2318}R", "\u{2318}/"] {
            assert!(card.iter().any(|(k, _)| k == key), "{key} is on the help card");
        }
        let mut e = HashMap::new();
        e.insert("help-record".to_string(), "Film it".to_string());
        let c = ScreenshotConfig::from_entries(&e);
        assert!(c.help_rows.iter().any(|(k, t)| k == "V" && t == "Film it"), "texts come from config");
        assert_eq!(c.shortcuts.len(), all.len());
    }

    #[test]
    fn cmd_slash_toggles_the_full_list_in_both_modes() {
        let mut s = session();
        s.handle_key(&ki(44, Some('/'), true, false));
        assert!(s.shortcuts_shown);
        s.handle_key(&ki(44, Some('/'), true, false));
        assert!(!s.shortcuts_shown, "again hides it");
        s.text_mode = true;
        s.handle_key(&ki(44, Some('/'), true, false));
        assert!(s.shortcuts_shown, "works in Copy Text mode too");
    }

    #[test]
    fn a_preset_area_opens_in_screenshot_mode_even_with_start_mode_text() {
        let mut cfg = ScreenshotConfig::default();
        cfg.start_text = true;
        let plain = ShotSession::new(cfg.clone(), ShotArgs::default(), temp_path("st1"));
        assert!(plain.text_mode, "Hyper+X with start-mode = text");
        let mut a = ShotArgs::default();
        a.last_region = true;
        let retake = ShotSession::new(cfg, a, temp_path("st2"));
        assert!(!retake.text_mode, "a retake / --last-region edits a screenshot");
    }

    #[test]
    fn only_shift_g_copies_the_hex() {
        let mut s = session();
        s.test_select(Rect::new(10.0, 10.0, 100.0, 80.0), Some(0));
        s.handle_key(&ki(5, Some('g'), false, false));
        assert!(s.grabbing.is_some() && !s.grab_copies, "G: pick only");
        s.cancel_grab();
        let mut shift_g = ki(5, Some('G'), false, false);
        shift_g.shift = true;
        s.handle_key(&shift_g);
        assert!(s.grabbing.is_some() && s.grab_copies, "Shift+G: pick and copy");
        s.cancel_grab();
        s.start_grab();
        assert!(!s.grab_copies, "the panel's Grab Color button never copies");
    }

    #[test]
    fn recording_frame_lands_on_the_right_display() {
        // Primary 1710x1107; a second display to its right, 1920x1080, top-aligned.
        let on_second = rec_frame_area(Rect::new(1800.0, 100.0, 400.0, 300.0), 1107.0, 0.0);
        assert_eq!((on_second.x, on_second.y), (1800.0, 707.0), "x unchanged, y flipped by the primary height");
        // A display above the primary: negative top-left y -> above 1107 in AppKit.
        let above = rec_frame_area(Rect::new(0.0, -1000.0, 200.0, 100.0), 1107.0, 3.0);
        assert_eq!((above.x, above.y, above.w, above.h), (-3.0, 2004.0, 206.0, 106.0));
    }

    #[test]
    fn ocr_accuracy_setting() {
        let mut e = HashMap::new();
        let c = ScreenshotConfig::from_entries(&e);
        assert!(!c.ocr.auto_accurate && !c.ocr.accurate, "fast by default (accurate fails on macOS 27 in the app)");
        e.insert("text-accuracy".to_string(), "auto".to_string());
        let c = ScreenshotConfig::from_entries(&e);
        assert!(c.ocr.auto_accurate && !c.ocr.accurate);
        e.insert("text-accuracy".to_string(), "Accurate".to_string());
        assert!(ScreenshotConfig::from_entries(&e).ocr.accurate);
        assert!(!crate::engines::screenshot_text::ShotOCR::use_accurate(false, false));
        assert!(crate::engines::screenshot_text::ShotOCR::use_accurate(true, false));
    }

    #[test]
    fn previews_fit_the_screen() {
        assert_eq!(pin_fit(400.0, 100.0, 1710.0, 1107.0), (400.0, 100.0), "small: real size");
        let (w, h) = pin_fit(1801.0, 4400.0, 1710.0, 1107.0);
        assert!(h <= 1107.0 * 0.8 + 0.5 && w < 1801.0, "a 200-line pane-shot is scaled down: {w}x{h}");
        assert!(((w / h) - 1801.0 / 4400.0).abs() < 0.01, "aspect kept");
    }

    #[test]
    fn barcode_and_hex_config_keys() {
        let mut e = HashMap::new();
        let c = ScreenshotConfig::from_entries(&e);
        assert!(c.ocr.barcodes && c.grab_copies_hex, "both on by default");
        e.insert("text-barcodes".to_string(), "false".to_string());
        e.insert("grab-copies-hex".to_string(), "false".to_string());
        e.insert("hex-toast".to_string(), "Hex {}".to_string());
        let c = ScreenshotConfig::from_entries(&e);
        assert!(!c.ocr.barcodes && !c.grab_copies_hex);
        assert_eq!(c.hex_toast, "Hex {}");
    }

    #[test]
    fn retake_delay_config_key() {
        let mut e = HashMap::new();
        e.insert("retake-delay".to_string(), "5".to_string());
        assert_eq!(ScreenshotConfig::from_entries(&e).retake_delay_ms, 5000);
    }

    fn panel_session() -> (ShotSession, PanelLayout) {
        let mut s = session();
        s.test_select(Rect::new(400.0, 200.0, 600.0, 400.0), Some(0));
        s.toggle_side_panel(Some(true));
        let l = s.panel_layout(0).expect("panel open on the active display");
        (s, l)
    }

    fn mid(r: Rect) -> Point {
        Point::new(r.mid_x(), r.mid_y())
    }

    #[test]
    fn side_panel_layout_stacks_controls_inside_the_panel() {
        let b = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let l = ShotSidePanel::layout(b, 24.0, 250.0);
        assert_eq!(l.panel, Rect::new(0.0, 0.0, 250.0, 900.0));
        let order = [
            l.size_label, l.size_value, l.slider, l.color_label, l.grab, l.wheel, l.hex,
            l.grid_box, l.grid_value,
        ];
        for w in order.windows(2) {
            assert!(w[0].max_y() <= w[1].y, "{:?} overlaps {:?}", w[0], w[1]);
        }
        for r in order {
            assert!(r.x >= 16.0 && r.max_x() <= 234.0, "{r:?} inside the padding");
        }
        assert!(l.grid_up.max_y() < 900.0, "everything fits a 900pt display");
        assert_eq!(l.wheel.w, 200.0, "wheel capped at 200");
    }

    #[test]
    fn side_panel_hit_test_maps_every_control() {
        let l = ShotSidePanel::layout(Rect::new(0.0, 0.0, 1440.0, 900.0), 24.0, 250.0);
        let h = |r: Rect| ShotSidePanel::hit(&l, mid(r));
        assert_eq!(h(l.size_down), Some(PanelHit::SizeDown));
        assert_eq!(h(l.size_up), Some(PanelHit::SizeUp));
        assert_eq!(h(l.slider), Some(PanelHit::Slider));
        assert_eq!(h(l.swatch), Some(PanelHit::Swatch));
        assert_eq!(h(l.grab), Some(PanelHit::Grab));
        assert_eq!(h(l.wheel), Some(PanelHit::Wheel));
        assert_eq!(h(l.hex), Some(PanelHit::Hex));
        assert_eq!(h(l.grid_box), Some(PanelHit::Grid));
        assert_eq!(h(l.grid_down), Some(PanelHit::GridDown));
        assert_eq!(h(l.grid_up), Some(PanelHit::GridUp));
        assert_eq!(ShotSidePanel::hit(&l, Point::new(240.0, 880.0)), Some(PanelHit::Body));
        assert_eq!(ShotSidePanel::hit(&l, Point::new(251.0, 100.0)), None, "outside");
        // A wheel-rect corner is outside the disc: background, not a colour.
        let corner = Point::new(l.wheel.x + 1.0, l.wheel.y + 1.0);
        assert_eq!(ShotSidePanel::hit(&l, corner), Some(PanelHit::Body));
    }

    #[test]
    fn side_panel_value_mapping() {
        let t = Rect::new(16.0, 0.0, 200.0, 20.0);
        assert_eq!(ShotSidePanel::slider_value(t, 16.0, 1, 100), 1);
        assert_eq!(ShotSidePanel::slider_value(t, 216.0, 1, 100), 100);
        assert_eq!(ShotSidePanel::slider_value(t, -50.0, 1, 100), 1, "clamped left");
        assert_eq!(ShotSidePanel::slider_value(t, 999.0, 0, 100), 100, "clamped right");
        for v in [1, 17, 50, 100] {
            let x = ShotSidePanel::slider_x(t, v, 1, 100);
            assert_eq!(ShotSidePanel::slider_value(t, x, 1, 100), v, "round trip {v}");
        }
        // Grid spin box: 5…50 step 5, off-step values snap down first.
        assert_eq!(ShotSidePanel::grid_step(10, 1), 15);
        assert_eq!(ShotSidePanel::grid_step(5, -1), 5);
        assert_eq!(ShotSidePanel::grid_step(50, 1), 50);
        assert_eq!(ShotSidePanel::grid_step(12, 1), 15);
        // Hex: valid applies, anything else reverts.
        let cur = ShotColor::new(1.0, 0.0, 0.0);
        assert_eq!(ShotSidePanel::hex_commit("#00ff00", cur), ShotColor::new(0.0, 1.0, 0.0));
        assert_eq!(ShotSidePanel::hex_commit("0000ff", cur), ShotColor::new(0.0, 0.0, 1.0));
        assert_eq!(ShotSidePanel::hex_commit("#0f0", cur), cur);
        assert_eq!(ShotSidePanel::hex_commit("zzzzzz", cur), cur);
        assert_eq!(ShotSidePanel::hex_commit("", cur), cur);
        // Wheel: centre = white, rim at angle 0 = pure red; the marker
        // lands back on the colour it was picked from.
        let w = Rect::new(0.0, 0.0, 200.0, 200.0);
        assert_eq!(ShotSidePanel::wheel_color(w, Point::new(100.0, 100.0)), Some(ShotColor::WHITE));
        assert_eq!(ShotSidePanel::wheel_color(w, Point::new(200.0, 100.0)).map(|c| c.hex()), Some("#ff0000".into()));
        assert_eq!(ShotSidePanel::wheel_color(w, Point::new(100.0, 400.0)), None);
        let c = ShotSidePanel::wheel_color(w, Point::new(140.0, 160.0)).unwrap();
        let m = ShotSidePanel::wheel_point(w, c);
        assert!((m.x - 140.0).abs() < 1.0 && (m.y - 160.0).abs() < 1.0, "{m:?}");
    }

    #[test]
    fn side_panel_size_controls_drive_the_next_stroke() {
        let (mut s, l) = panel_session();
        assert_eq!(s.panel_size(), None, "no tool: the size control is idle");
        assert!(s.panel_click(mid(l.size_up)), "click swallowed by the panel");
        assert_eq!(s.panel_size(), None);
        s.set_tool(Some(ShotTool::Pencil));
        assert_eq!(s.panel_size(), Some((3, 1, 100)));
        s.panel_click(mid(l.size_up));
        s.panel_click(mid(l.size_up));
        s.panel_click(mid(l.size_down));
        assert_eq!(s.size(ShotTool::Pencil), 4);
        // Slider: drag from the left end to the right end.
        s.mouse_down(0, Point::new(l.slider.x, l.slider.mid_y()), 1, false, false);
        assert_eq!(s.size(ShotTool::Pencil), 1);
        s.mouse_dragged(0, Point::new(l.slider.max_x() + 40.0, l.slider.mid_y()), false, false);
        s.mouse_up(0, Point::new(l.slider.max_x() + 40.0, l.slider.mid_y()), false, false);
        assert_eq!(s.size(ShotTool::Pencil), 100);
        assert!(s.selection.is_some(), "the drag never touched the selection");
        assert_eq!(s.selection.unwrap().x, 400.0);
        // Per-tool memory: another tool keeps its own size.
        s.set_tool(Some(ShotTool::Marker));
        assert_eq!(s.panel_size().map(|v| v.0), Some(5));
        s.set_tool(Some(ShotTool::Pencil));
        s.test_draw(Point::new(500.0, 300.0), Point::new(600.0, 350.0));
        assert_eq!(s.doc.objects.last().map(|o| o.size), Some(100));
    }

    #[test]
    fn side_panel_wheel_and_hex_change_the_colour() {
        let (mut s, l) = panel_session();
        let rim = Point::new(l.wheel.max_x(), l.wheel.mid_y());
        s.mouse_down(0, rim, 1, false, false);
        assert_eq!(s.color.hex(), "#ff0000", "live while dragging");
        s.mouse_up(0, rim, false, false);
        assert_eq!(s.state.color.as_deref(), Some("#ff0000"), "persisted on release");

        // Hex: focus selects all, typing replaces, Return applies.
        s.panel_click(mid(l.hex));
        assert_eq!(s.hex_edit.as_ref().map(|h| h.replace), Some(true));
        for c in "00ff00".chars() {
            assert!(s.handle_key(&ki(0, Some(c), false, false)));
        }
        assert_eq!(s.hex_edit.as_ref().map(|h| h.text.as_str()), Some("00ff00"));
        s.handle_key(&ki(KEY_RETURN, None, false, false));
        assert!(s.hex_edit.is_none());
        assert_eq!(s.color.hex(), "#00ff00");

        // Invalid input reverts; tool letters typed in the field stay there.
        s.panel_click(mid(l.hex));
        s.handle_key(&ki(0, Some('p'), false, false));
        assert_eq!(s.tool, None, "a letter in the field never picks a tool");
        s.handle_key(&ki(0, Some('a'), false, false));
        s.handle_key(&ki(KEY_RETURN, None, false, false));
        assert_eq!(s.color.hex(), "#00ff00", "'a' is not a colour: reverted");

        // Esc inside the field leaves the field only (rule 6).
        s.panel_click(mid(l.hex));
        s.handle_key(&ki(KEY_ESC, None, false, false));
        assert!(s.hex_edit.is_none());
        assert!(s.side_panel_open, "panel still open");
        assert!(!s.finished, "overlay still open");
        s.handle_key(&ki(KEY_ESC, None, false, false));
        assert!(!s.side_panel_open, "next Esc closes the panel");
        assert!(!s.finished);
    }

    #[test]
    fn side_panel_hex_field_honours_edit_shortcuts() {
        let (mut s, l) = panel_session();
        s.panel_click(mid(l.hex));
        // Cmd+X empties (and copies), Ctrl+Z restores, Backspace edits.
        s.handle_key(&ki(KEY_DELETE, None, false, false));
        assert_eq!(s.hex_edit.as_ref().unwrap().text, "", "select-all + delete clears");
        s.handle_key(&ki(0, Some('z'), false, true));
        assert_eq!(s.hex_edit.as_ref().unwrap().text, s.current_color().hex());
        s.handle_key(&ki(0, Some('a'), true, false));
        assert!(s.hex_edit.as_ref().unwrap().replace, "Cmd+A selects all");
        // Typing past 7 characters is ignored.
        for c in "#1234567".chars() {
            s.handle_key(&ki(0, Some(c), false, false));
        }
        assert_eq!(s.hex_edit.as_ref().unwrap().text, "#123456");
        s.handle_key(&ki(KEY_DELETE, None, false, false));
        assert_eq!(s.hex_edit.as_ref().unwrap().text, "#12345");
        // Clicking elsewhere commits (focus-out).
        s.handle_key(&ki(0, Some('6'), false, false));
        s.mouse_down(0, Point::new(700.0, 400.0), 1, false, false);
        s.mouse_up(0, Point::new(700.0, 400.0), false, false);
        assert!(s.hex_edit.is_none());
        assert_eq!(s.color.hex(), "#123456");
    }

    #[test]
    fn side_panel_grab_hides_and_returns() {
        let (mut s, l) = panel_session();
        let before = s.color;
        s.panel_click(mid(l.grab));
        assert!(s.grabbing.is_some(), "grab started");
        assert!(!s.side_panel_open, "panel hidden during the grab");
        assert!(s.tool_tab_rect(0).is_none(), "no tab during the grab either");
        s.color = ShotColor::new(0.1, 0.2, 0.3); // a live preview
        s.escape();
        assert!(s.grabbing.is_none());
        assert_eq!(s.color, before, "Esc restores the previous colour");
        assert!(s.side_panel_open, "panel comes back after the grab");
        // A grab from the G key (not the panel) leaves the panel alone.
        s.toggle_side_panel(Some(false));
        s.start_grab();
        s.escape();
        assert!(!s.side_panel_open);
    }

    #[test]
    fn side_panel_grid_and_tab() {
        let mut s = session();
        s.test_select(Rect::new(400.0, 200.0, 600.0, 400.0), Some(0));
        let tab = s.tool_tab_rect(0).expect("tab shown while the panel is closed");
        assert_eq!(tab.x, 0.0);
        assert!(s.panel_click(mid(tab)));
        assert!(s.side_panel_open, "the tab opens the panel");
        assert!(s.tool_tab_rect(0).is_none());
        let l = s.panel_layout(0).unwrap();
        assert!(!s.state.grid);
        s.panel_click(mid(l.grid_box));
        assert!(s.state.grid);
        s.panel_click(mid(l.grid_up));
        assert_eq!(s.state.grid_size, 15);
        for _ in 0..20 {
            s.panel_click(mid(l.grid_down));
        }
        assert_eq!(s.state.grid_size, 5);
        assert_eq!(ShotState::load(&s.state_path).grid_size, 5, "persisted");
        // Text mode hides both the panel and the tab.
        s.toggle_text_mode();
        assert!(s.panel_layout(0).is_none() && s.tool_tab_rect(0).is_none());
        let mut cfg = ScreenshotConfig::default();
        cfg.show_side_panel_button = false;
        let mut t = ShotSession::new(cfg, ShotArgs::default(), temp_path("tab"));
        t.add_display(ShotDisplay::new(1, Rect::new(0.0, 0.0, 1440.0, 900.0), 2.0));
        assert!(t.tool_tab_rect(0).is_none(), "show-side-panel-button = false");
    }

    #[test]
    fn side_panel_config_keys() {
        let mut e = HashMap::new();
        e.insert("side-panel-width".to_string(), "300".to_string());
        e.insert("side-panel-grab-label".to_string(), "Pick".to_string());
        e.insert("draw-thickness".to_string(), "7".to_string());
        e.insert("draw-font-size".to_string(), "20".to_string());
        let c = ScreenshotConfig::from_entries(&e);
        assert_eq!(c.side_panel_width, 300.0);
        assert_eq!(c.panel_labels.grab, "Pick");
        assert_eq!(c.panel_labels.size, "Active tool size:");
        let mut s = ShotSession::new(c, ShotArgs::default(), temp_path("cfgsz"));
        assert_eq!(s.size(ShotTool::Line), 7, "draw-thickness seeds line tools");
        assert_eq!(s.size(ShotTool::Text), 20, "draw-font-size seeds text");
        assert_eq!(s.size(ShotTool::Marker), 5, "marker keeps its own default");
        s.state.sizes.insert("line".to_string(), 2);
        assert_eq!(s.size(ShotTool::Line), 2, "a remembered size wins");
    }

    #[test]
    fn merge_layout_stacks_in_order_widest_sets_the_column() {
        // A wide shot, a narrow one, a medium one: gutter 88, gap 32, pad 32.
        let l = merge_layout(&[(1000.0, 400.0), (300.0, 200.0), (600.0, 500.0)], false, 88.0, 32.0, 32.0);
        assert_eq!(l.width, 32.0 + 88.0 + 1000.0 + 32.0, "column = widest shot");
        assert_eq!(l.height, 32.0 + 400.0 + 32.0 + 200.0 + 32.0 + 500.0 + 32.0);
        // Top to bottom in pick order, left-aligned after the gutter, native size.
        assert_eq!(l.images[0], Rect::new(120.0, 32.0, 1000.0, 400.0));
        assert_eq!(l.images[1], Rect::new(120.0, 464.0, 300.0, 200.0), "narrow: not upscaled");
        assert_eq!(l.images[2], Rect::new(120.0, 696.0, 600.0, 500.0));
        // Each number cell sits left of its own row, row-high.
        assert_eq!(l.labels[1], Rect::new(32.0, 464.0, 88.0, 200.0));
    }

    #[test]
    fn merge_layout_horizontal_runs_left_to_right_tallest_sets_the_row() {
        let l = merge_layout(&[(1000.0, 400.0), (300.0, 200.0)], true, 88.0, 32.0, 32.0);
        assert_eq!(l.width, 32.0 + 1000.0 + 32.0 + 300.0 + 32.0);
        assert_eq!(l.height, 32.0 + 88.0 + 400.0 + 32.0, "row = tallest + number strip");
        assert_eq!(l.images[0], Rect::new(32.0, 120.0, 1000.0, 400.0));
        assert_eq!(l.images[1], Rect::new(1064.0, 120.0, 300.0, 200.0), "top-aligned, native");
        assert_eq!(l.labels[1], Rect::new(1064.0, 32.0, 300.0, 88.0), "number strip above");
    }

    #[test]
    fn merge_stage_keeps_native_pixels_or_shrinks_to_fit() {
        // A small stack on a 1440x900 @2x display: native, at the display scale.
        let st = merge_stage(800.0, 600.0, 1440.0, 900.0, 25.0, 48.0, 2.0, 3.0);
        assert_eq!((st.scale, st.fit), (2.0, 1.0));
        assert_eq!((st.rect.w, st.rect.h), (400.0, 300.0), "800x600 px at 2 px/pt");
        assert!(st.rect.y >= 25.0 + 48.0, "below the menu bar + margin");
        // Taller than the screen at 2x: more pixels per point, still native.
        let st = merge_stage(1200.0, 2000.0, 1440.0, 900.0, 25.0, 48.0, 2.0, 3.0);
        assert_eq!(st.fit, 1.0);
        assert!(st.scale > 2.0 && st.scale <= 3.0);
        assert!(st.rect.h <= 900.0 - 25.0 - 96.0 + 0.01, "fits the screen");
        // Too tall even at max-scale: shrunk, and still fits.
        let st = merge_stage(3000.0, 9000.0, 1440.0, 900.0, 25.0, 48.0, 2.0, 3.0);
        assert_eq!(st.scale, 3.0);
        assert!(st.fit < 1.0);
        assert!(st.rect.h <= 900.0 - 25.0 - 96.0 + 0.01);
    }

    #[test]
    fn merge_config_keys() {
        let mut e = HashMap::new();
        e.insert("merge-gap".to_string(), "8".to_string());
        e.insert("merge-gutter".to_string(), "60".to_string());
        e.insert("merge-max-scale".to_string(), "9".to_string());
        e.insert("merge-background".to_string(), "#ffffff".to_string());
        e.insert("merge-hint".to_string(), "".to_string());
        let c = ScreenshotConfig::from_entries(&e);
        assert_eq!((c.merge.gap, c.merge.gutter), (8.0, 60.0));
        assert_eq!(c.merge.max_scale, 6.0, "clamped");
        assert_eq!(c.merge.background, ShotColor::from_hex("#ffffff").unwrap());
        assert_eq!(c.merge.hint, "", "an empty hint hides it");
        assert_eq!(c.merge.label_size, 18.0, "default");
        assert_eq!(c.merge.label_color, ShotColor::from_hex("#ffc400").unwrap(), "amber, not the ui colour");
        assert_eq!(c.merge.border_width, 1.0);
        assert!(!c.merge.horizontal, "vertical by default");
        e.insert("merge-direction".to_string(), "horizontal".to_string());
        assert!(ScreenshotConfig::from_entries(&e).merge.horizontal);
    }

    #[test]
    fn merge_labels_keep_their_numbers_when_one_is_deleted() {
        let mut d = ShotDocument::new(10);
        d.objects = (1..=3)
            .map(|n| {
                let mut o = ShotObject::new(ShotTool::Counter, vec![Point::new(0.0, n as f64 * 50.0)], ShotColor::WHITE, 3);
                o.number = n;
                o.fixed_number = true;
                o
            })
            .collect();
        d.remove(1); // delete "2"
        let nums: Vec<i64> = d.objects.iter().map(|o| o.number).collect();
        assert_eq!(nums, vec![1, 3], "3 stays 3");
        // A counter the user adds numbers on its own, from 1.
        d.add(ShotObject::new(ShotTool::Counter, vec![Point::new(9.0, 9.0)], ShotColor::WHITE, 3));
        assert_eq!(d.objects.last().map(|o| o.number), Some(1));
    }

    #[test]
    fn move_item_and_the_drag_preview_shift_the_rest() {
        let mut v = vec!["a", "b", "c", "d"];
        move_item(&mut v, 0, 2);
        assert_eq!(v, ["b", "c", "a", "d"], "1 to slot 3: 2 and 3 move up");
        move_item(&mut v, 3, 0);
        assert_eq!(v, ["d", "b", "c", "a"]);
        move_item(&mut v, 1, 9);
        assert_eq!(v, ["d", "b", "c", "a"], "out of range: no-op");
        assert_eq!(merge_preview_order(4, None), [0, 1, 2, 3]);
        assert_eq!(merge_preview_order(4, Some((0, 2))), [1, 2, 0, 3]);
    }

    #[test]
    fn merge_strip_rows_fit_and_map_drags_to_slots() {
        let b = Rect::new(0.0, 0.0, 1440.0, 900.0);
        assert!(MergeStrip::layout(b, 25.0, 30.0, 1).is_none(), "one photo: no strip");
        let l = MergeStrip::layout(b, 25.0, 30.0, 3).unwrap();
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.rows[0].h, MergeStrip::ROW, "few photos: full-size rows");
        assert!(l.strip.y >= 25.0 && l.strip.max_y() <= 900.0);
        assert_eq!(l.strip.x, 30.0);
        let mid = |r: Rect| Point::new(r.mid_x(), r.mid_y());
        assert_eq!(MergeStrip::row_at(&l, mid(l.rows[1])), Some(1));
        assert_eq!(MergeStrip::row_at(&l, mid(l.hint)), None);
        assert_eq!(MergeStrip::slot_at(&l, l.rows[2].mid_y()), 2);
        assert_eq!(MergeStrip::slot_at(&l, -100.0), 0, "above: first slot");
        assert_eq!(MergeStrip::slot_at(&l, 5000.0), 2, "below: last slot");
        // Many photos shrink the rows so the strip still fits the screen.
        let l = MergeStrip::layout(b, 25.0, 30.0, 20).unwrap();
        assert!(l.rows[0].h < MergeStrip::ROW);
        assert!(l.strip.max_y() <= 900.0 + 0.01);
        // The stack is centred right of the strip's reserve.
        let st = merge_stage_left(800.0, 600.0, 1440.0, 900.0, 25.0, MergeStrip::reserve(), 48.0, 2.0, 3.0);
        assert!(st.rect.x >= MergeStrip::reserve() + 48.0 - 0.01);
        assert_eq!(merge_stage_left(800.0, 600.0, 1440.0, 900.0, 25.0, 0.0, 48.0, 2.0, 3.0), merge_stage(800.0, 600.0, 1440.0, 900.0, 25.0, 48.0, 2.0, 3.0));
    }

    /// Drag photo 1 onto slot 3 on the strip: the stack is rebuilt in the
    /// new order, numbers stay 1-2-3 top to bottom, and an arrow drawn on
    /// photo 1 travels with it.
    #[cfg(target_os = "macos")]
    #[test]
    fn dragging_a_strip_row_reorders_the_merged_stack() {
        let solid = |w: i64, h: i64| {
            let st = MergeStage { scale: 1.0, fit: 1.0, rect: Rect::new(0.0, 0.0, w as f64, h as f64) };
            shot_cg::merge_canvas(&[], w, h, &st, &merge_layout(&[], false, 0.0, 0.0, 0.0), ShotColor::WHITE, ShotColor::WHITE, 0.0).unwrap()
        };
        let images = vec![solid(400, 100), solid(300, 200), solid(200, 50)];
        let sizes: Vec<(f64, f64)> = images
            .iter()
            .map(|i| (objc2_core_graphics::CGImage::width(Some(i)) as f64, objc2_core_graphics::CGImage::height(Some(i)) as f64))
            .collect();
        let cfg = ScreenshotConfig::default();
        let m = cfg.merge.clone();
        let px = 2.0;
        let layout = merge_layout(&sizes, false, m.gutter * px, m.gap * px, m.gap * px);
        let stage = merge_stage_left(layout.width, layout.height, 1440.0, 900.0, 25.0, MergeStrip::reserve(), 48.0, 2.0, m.max_scale);
        let mut s = ShotSession::new(cfg.clone(), ShotArgs::default(), temp_path("mergemove"));
        let mut disp = ShotDisplay::new(1, Rect::new(0.0, 0.0, 1440.0, 900.0), 2.0);
        disp.top_inset = 25.0;
        s.add_display(disp);
        s.test_select(stage.rect, Some(0));
        s.doc.objects = merge_labels(&stage, &layout, &m, px, false);
        let k = stage.fit / stage.scale;
        let pt = |r: Rect| Rect::new(stage.rect.x + r.x * k, stage.rect.y + r.y * k, r.w * k, r.h * k);
        let first = pt(layout.images[0]);
        let a = Point::new(first.x + 10.0, first.y + 10.0);
        s.doc.add(ShotObject::new(ShotTool::Arrow, vec![a, Point::new(a.x + 20.0, a.y + 10.0)], ShotColor::WHITE, 3));
        s.merge = Some(MergeEdit {
            paths: vec!["1.png".into(), "2.png".into(), "3.png".into()],
            images,
            horizontal: false,
            px,
            layout: layout.clone(),
            stage: stage.clone(),
            canvas: ((1440.0 * stage.scale) as i64, (900.0 * stage.scale) as i64),
            display: 0,
            drag: None,
        });
        assert!(s.doc.can_undo());
        let l = s.merge_strip(0).expect("strip shown for 3 photos");
        assert!(l.strip.max_x() <= stage.rect.x, "strip clear of the stack");
        let mid = |r: Rect| Point::new(r.mid_x(), r.mid_y());
        s.mouse_down(0, mid(l.rows[0]), 1, false, false);
        s.mouse_dragged(0, mid(l.rows[2]), false, false);
        assert_eq!(s.merge.as_ref().unwrap().drag, Some((0, 2)), "live preview while dragging");
        s.mouse_up(0, mid(l.rows[2]), false, false);
        let me = s.merge.as_ref().unwrap();
        assert_eq!(me.paths, ["2.png", "3.png", "1.png"]);
        assert!(me.drag.is_none() && s.base_dirty);
        assert_eq!(s.selection, Some(stage.rect), "same photos, same stage");
        // Numbers: still 1, 2, 3 and at the new slots' cells.
        let want = merge_labels(&stage, &me.layout, &m, px, false);
        for n in 1..=3i64 {
            let o = s.doc.objects.iter().find(|o| o.fixed_number && o.number == n).unwrap();
            assert_eq!(o.points, want[n as usize - 1].points, "label {n}");
        }
        // The arrow moved with photo 1 (now the last slot).
        let last = pt(me.layout.images[2]);
        let arrow = s.doc.objects.iter().find(|o| o.tool == ShotTool::Arrow).unwrap();
        assert_eq!(arrow.points[0], Point::new(last.x + 10.0, last.y + 10.0));
        assert!(!s.doc.can_undo(), "undo cleared with the old canvas");
        // A click on a row without moving changes nothing.
        assert!(!s.move_merged(1, 1));

        // The strip's → Horizontal segment restacks left to right in place.
        s.base_dirty = false;
        let l = s.merge_strip(0).unwrap();
        s.mouse_down(0, mid(l.direction[1]), 1, false, false);
        s.mouse_up(0, mid(l.direction[1]), false, false);
        let me = s.merge.as_ref().unwrap();
        assert!(me.horizontal && s.base_dirty);
        assert_eq!(me.paths, ["2.png", "3.png", "1.png"], "order kept");
        let ys: Vec<f64> = me.layout.images.iter().map(|r| r.y).collect();
        assert!(ys.iter().all(|y| *y == ys[0]), "one row");
        assert_eq!(s.selection, Some(me.stage.rect), "selection = the new stack");
        let want = merge_labels(&me.stage, &me.layout, &m, px, false);
        let one = s.doc.objects.iter().find(|o| o.fixed_number && o.number == 1).unwrap();
        assert_eq!(one.points, want[0].points);
        let k = me.stage.fit / me.stage.scale;
        let st = me.stage.clone();
        let last = Rect::new(st.rect.x + me.layout.images[2].x * k, st.rect.y + me.layout.images[2].y * k, 1.0, 1.0);
        let arrow = s.doc.objects.iter().find(|o| o.tool == ShotTool::Arrow).unwrap();
        assert!((arrow.points[0].x - last.x) > 0.0 && (arrow.points[0].y - last.y) > 0.0, "arrow still on photo 1");
        // ↓ Vertical goes back; pressing the lit segment is a no-op.
        assert!(!s.set_merge_horizontal(Some(true)));
        assert!(s.set_merge_horizontal(None));
        assert!(!s.merge.as_ref().unwrap().horizontal);
    }

    /// Paint the strip offscreen (no window, nothing on screen) mid-drag
    /// and check it drew; `KS_STRIP_PNG=out.png` saves it to look at.
    #[cfg(target_os = "macos")]
    #[test]
    fn merge_strip_paints_offscreen() {
        use objc2_core_graphics::{CGBitmapContextCreateImage, CGContext};
        let solid = |w: i64, h: i64, c: ShotColor| {
            let st = MergeStage { scale: 1.0, fit: 1.0, rect: Rect::new(0.0, 0.0, w as f64, h as f64) };
            shot_cg::merge_canvas(&[], w, h, &st, &merge_layout(&[], false, 0.0, 0.0, 0.0), c, c, 0.0).unwrap()
        };
        let mut s = ShotSession::new(ScreenshotConfig::default(), ShotArgs::default(), temp_path("strippaint"));
        s.add_display(ShotDisplay::new(1, Rect::new(0.0, 0.0, 400.0, 500.0), 1.0));
        let images = vec![
            solid(400, 100, ShotColor::from_hex("#dd3333").unwrap()),
            solid(300, 300, ShotColor::from_hex("#33aa33").unwrap()),
            solid(200, 50, ShotColor::from_hex("#3366dd").unwrap()),
        ];
        let st = MergeStage { scale: 1.0, fit: 1.0, rect: Rect::new(0.0, 0.0, 1.0, 1.0) };
        s.merge = Some(MergeEdit {
            paths: vec!["a".into(), "b".into(), "c".into()],
            images,
            horizontal: false,
            px: 1.0,
            layout: merge_layout(&[], false, 0.0, 0.0, 0.0),
            stage: st,
            canvas: (1, 1),
            display: 0,
            drag: Some((0, 2)),
        });
        let l = s.merge_strip(0).unwrap();
        let ctx = shot_cg::bitmap_context(400, 500).unwrap();
        // The overlay is a flipped view: top-left origin.
        CGContext::translate_ctm(Some(&ctx), 0.0, 500.0);
        CGContext::scale_ctm(Some(&ctx), 1.0, -1.0);
        overlay::draw_merge_strip(&ctx, &s, &l);
        let img = CGBitmapContextCreateImage(Some(&ctx)).unwrap();
        let png = shot_cg::encode_png(&img).unwrap();
        assert!(png.len() > 2000, "the strip drew something");
        if let Ok(out) = std::env::var("KS_STRIP_PNG") {
            std::fs::write(out, &png).unwrap();
        }
    }

    #[test]
    fn editor_menu_titles_come_from_config() {
        let mut e = HashMap::new();
        let c = ScreenshotConfig::from_entries(&e);
        assert_eq!(editor_menu_title(&c.merge, 1), "Open in Screenshot Editor");
        assert_eq!(editor_menu_title(&c.merge, 3), "Merge 3 Photos in Screenshot Editor");
        e.insert("editor-merge-menu-title".to_string(), "Stack {}".to_string());
        assert_eq!(editor_menu_title(&ScreenshotConfig::from_entries(&e).merge, 2), "Stack 2");
    }

    /// Real decode: a PNG round-trips at native size; a text file named
    /// `.png` is refused. `KS_DECODE_PROBE=a.heic:b.jpg` also decodes those
    /// and prints their upright size (manual EXIF-orientation check).
    #[cfg(target_os = "macos")]
    #[test]
    fn decode_image_file_reads_pictures_and_refuses_fakes() {
        let dir = std::env::temp_dir().join(format!("ks-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = shot_cg::merge_canvas(&[], 30, 10, &MergeStage { scale: 1.0, fit: 1.0, rect: Rect::new(0.0, 0.0, 30.0, 10.0) },
            &merge_layout(&[], false, 0.0, 0.0, 0.0), ShotColor::WHITE, ShotColor::WHITE, 0.0).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, shot_cg::encode_png(&img).unwrap()).unwrap();
        let got = shot_cg::decode_image_file(&png.to_string_lossy(), EDITOR_MAX_IMAGE_PX).expect("png decodes");
        let size = |i: &objc2_core_graphics::CGImage| (objc2_core_graphics::CGImage::width(Some(i)), objc2_core_graphics::CGImage::height(Some(i)));
        assert_eq!(size(&got), (30, 10), "native size, not resampled");
        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"not a picture at all").unwrap();
        assert!(shot_cg::decode_image_file(&fake.to_string_lossy(), EDITOR_MAX_IMAGE_PX).is_none());
        for p in std::env::var("KS_DECODE_PROBE").unwrap_or_default().split(':').filter(|p| !p.is_empty()) {
            let i = shot_cg::decode_image_file(p, EDITOR_MAX_IMAGE_PX);
            eprintln!("probe {p}: {:?}", i.as_deref().map(size));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn esc_in_the_merge_editor_returns_to_the_original_capture() {
        let mut c = ScreenshotController::new();
        let mut parent = session();
        parent.test_select(Rect::new(5.0, 5.0, 50.0, 50.0), Some(0));
        let mut merged = session();
        merged.finish(ShotOutcome::Abort);
        c.session = Some(merged);
        c.merge_parent = Some((parent, vec!["a.png".to_string()], false));
        c.complete_finished_session();
        let s = c.session.as_ref().expect("the original capture is back");
        assert!(!s.finished);
        assert_eq!(s.selection.map(|r| r.w), Some(50.0));
        assert!(c.merge_parent.is_none());
        // A real abort from there closes for good.
        c.abort_session();
        assert!(c.session.is_none());
    }

    #[test]
    fn finish_waits_for_an_in_flight_capture() {
        let mut c = ScreenshotController::new();
        let mut s = session();
        s.test_select(Rect::new(10.0, 10.0, 100.0, 100.0), Some(0));
        s.finish(ShotOutcome::Text);
        c.session = Some(s);
        c.capture_pending = true;
        c.complete_finished_session();
        assert!(c.session.is_some(), "Return before the image lands waits");
        c.capture_pending = false;
        c.complete_finished_session();
        assert!(c.session.is_none(), "completes once the capture is in");
        // Esc never waits.
        let mut s = session();
        s.finish(ShotOutcome::Abort);
        c.session = Some(s);
        c.capture_pending = true;
        c.complete_finished_session();
        assert!(c.session.is_none());
    }

    #[test]
    fn switching_copy_text_and_screenshot_starts_clean() {
        let mut s = session();
        s.goto_stop(1);
        assert!(!s.text_mode);
        s.test_select(Rect::new(100.0, 100.0, 200.0, 200.0), Some(0));
        s.doc.add(ShotObject::new(
            ShotTool::Rectangle,
            vec![Point::new(110.0, 110.0), Point::new(150.0, 150.0)],
            ShotColor::BLACK,
            3,
        ));
        // Shift+Tab back to Copy Text: no leftover region or annotations.
        s.cycle_stop(true);
        assert!(s.text_mode);
        assert!(s.selection.is_none(), "selection cleared on mode switch");
        assert!(s.doc.objects.is_empty(), "annotations cleared on mode switch");
        // Re-picking the current stop keeps the selection.
        s.test_select(Rect::new(10.0, 10.0, 50.0, 50.0), Some(0));
        s.goto_stop(0);
        assert!(s.selection.is_some());
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
            sel: None,
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
        assert!(!s.finished);
        s.escape();
        assert!(s.finished, "7: overlay finishes (abort) despite tool/selection");
        assert_eq!(s.outcome, Some(ShotOutcome::Abort));
    }

    #[test]
    fn session_key_routing_tools_text_mode() {
        let mut s = session();
        s.test_select(Rect::new(10.0, 10.0, 100.0, 100.0), Some(0));

        // Tool letter.
        assert!(s.handle_key(&shot_key_event("p").unwrap()));
        assert_eq!(s.tool, Some(ShotTool::Pencil));
        // Esc with a tool armed cancels the capture.
        assert!(s.handle_key(&shot_key_event("esc").unwrap()));
        assert!(s.finished);
        let mut s = session();
        s.test_select(Rect::new(10.0, 10.0, 100.0, 100.0), Some(0));

        // Tab cycles the pill stops (Screenshot → Recent); it no longer toggles
        // text mode (the audit's "changed by design"). `O` toggles Copy Text.
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
        assert!(s.recent_requested, "Tab reaches the recents stop");
        assert!(!s.text_mode);
        assert!(s.handle_key(&shot_key_event("o").unwrap()));
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
            20,
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
        assert!(c.test_do("recents").is_err(), "recents needs an overlay");
        assert!(c.test_do("recents:select:0").is_err(), "recents not open");
        assert!(c.test_do("recents:bogus").is_err());

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
        assert_eq!(ocr_text(&bad, &ShotOCRConfigModel::default()).text, "");
        let none = ShotImage::new(10, 10);
        assert_eq!(ocr_text(&none, &ShotOCRConfigModel::default()).text, "");
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

    #[test]
    fn right_click_color_wheel_picks_and_opens_panel() {
        let mut s = session();
        s.cfg.user_colors = vec![
            Some(ShotColor::new(0.0, 1.0, 0.0)),
            Some(ShotColor::new(0.0, 0.0, 1.0)),
            None,
        ];
        let c = Point::new(400.0, 400.0);

        // rightDown opens the wheel centred at the pointer; the hub hits nothing.
        s.right_down(0, c);
        let w = s.wheel.as_ref().unwrap();
        assert_eq!((w.display, w.center, w.hot), (0, c, None));
        assert_eq!(w.hit(c, 3), None, "the hub is not a swatch");

        // rightDragged tracks the nearest swatch; rightUp applies its colour.
        let d0 = ShotWheel::dot_center(c, 0, 3);
        s.right_dragged(0, d0);
        assert_eq!(s.wheel.as_ref().unwrap().hot, Some(0));
        s.right_up(0, d0);
        assert!(s.wheel.is_none(), "picking closes the wheel");
        assert_eq!(s.color, ShotColor::new(0.0, 1.0, 0.0));

        // The picker swatch (None) opens the side panel instead.
        let d2 = ShotWheel::dot_center(c, 2, 3);
        s.right_down(0, c);
        s.right_dragged(0, d2);
        s.right_up(0, d2);
        assert!(s.wheel.is_none());
        assert!(s.side_panel_open, "picker swatch opens the side panel");

        // A drag on another display clears `hot` (the wheel's own point space).
        s.right_down(0, c);
        s.right_dragged(1, d0);
        assert_eq!(s.wheel.as_ref().unwrap().hot, None);
    }

    #[test]
    fn text_typing_edits_and_commits_the_draft() {
        let mut s = session();
        s.test_select(Rect::new(10.0, 10.0, 200.0, 200.0), Some(0));
        s.begin_text(0, Point::new(50.0, 50.0), None);
        assert!(s.editing.is_some());

        assert!(s.handle_key(&shot_key_event("a").unwrap()));
        assert!(s.handle_key(&shot_key_event("b").unwrap()));
        assert_eq!(s.editing.as_ref().unwrap().text, "ab");
        // The painted draft (`object.text`) mirrors the editable buffer.
        assert_eq!(s.editing.as_ref().unwrap().object.text, "ab");

        assert!(s.handle_key(&shot_key_event("delete").unwrap()));
        assert_eq!(s.editing.as_ref().unwrap().text, "a", "backspace deletes");

        assert!(s.handle_key(&shot_key_event("return").unwrap()));
        assert_eq!(s.editing.as_ref().unwrap().text, "a\n", "Return = newline");

        assert!(s.handle_key(&shot_key_event("cmd+return").unwrap()));
        assert!(s.editing.is_none(), "Cmd+Return commits");
        assert_eq!(s.doc.objects.len(), 1);
        assert_eq!(s.doc.objects[0].text, "a", "trailing newline trimmed");

        // Empty text is dropped like Swift's `isValid == false`.
        s.begin_text(0, Point::new(5.0, 5.0), None);
        s.commit_text();
        assert_eq!(s.doc.objects.len(), 1, "empty draft adds nothing");
    }

    #[test]
    fn save_card_selection_replaces_stem_and_select_all() {
        let mut c = SaveCardState { display: 0, field: "~/Shots/a.png".into(), sel: None };
        let (loc, len) = ShotSaveCard::focus_range(&c.field);
        c.sel = Some((loc, loc + len));
        c.insert("new");
        assert_eq!(c.field, "~/Shots/new.png");
        c.select_all();
        c.insert("/tmp/x.png");
        assert_eq!(c.field, "/tmp/x.png");
        c.select_all();
        c.backspace();
        assert_eq!(c.field, "");
    }

    #[test]
    fn save_card_field_is_typed_here() {
        let mut s = session();
        s.test_select(Rect::new(0.0, 0.0, 100.0, 100.0), Some(0));
        s.save_card = Some(SaveCardState {
            display: 0,
            field: "/tmp/x.png".to_string(),
            sel: None,
        });
        assert!(s.handle_key(&shot_key_event("a").unwrap()));
        assert_eq!(s.save_card.as_ref().unwrap().field, "/tmp/x.pnga");
        assert!(s.handle_key(&shot_key_event("delete").unwrap()));
        assert_eq!(s.save_card.as_ref().unwrap().field, "/tmp/x.png");
        assert!(s.handle_key(&shot_key_event("return").unwrap()));
        assert!(s.save_card.is_none(), "Return confirms");
        assert_eq!(s.chosen_save_path.as_deref(), Some("/tmp/x.png"));
    }

    #[test]
    fn ring_button_press_fires_on_release() {
        let frames = vec![Rect::ZERO, Rect::new(10.0, 10.0, 20.0, 20.0)];
        assert_eq!(ring_index_at(&frames, Point::new(15.0, 15.0)), Some(1));
        assert_eq!(ring_index_at(&frames, Point::new(0.0, 0.0)), None);

        let mut s = session();
        s.test_select(Rect::new(300.0, 300.0, 200.0, 200.0), Some(0));
        let layout = ButtonRing::layout(
            s.selection.unwrap(),
            s.displays[0].bounds(),
            s.cfg.buttons.len(),
            s.cfg.button_size,
        );
        let f = layout.frames[0];
        let p = Point::new(f.mid_x(), f.mid_y());
        let first = s.cfg.buttons[0];

        s.mouse_down(0, p, 1, false, false);
        assert_eq!(s.chrome_press, Some(ChromePress::Ring(0)));
        assert_eq!(s.tool, None, "no fire before release");
        s.mouse_up(0, p, false, false);
        assert_eq!(s.tool, Some(first), "release inside the frame fires");

        // A release outside the frame does nothing (the press is swallowed).
        s.set_tool(None);
        s.mouse_down(0, p, 1, false, false);
        s.mouse_up(0, Point::new(0.0, 0.0), false, false);
        assert_eq!(s.tool, None, "release outside the frame does not fire");
    }

    #[test]
    fn initial_region_maps_screen_rect_and_last_region() {
        let mut s = session();
        // A second display to the right of the primary.
        s.add_display(ShotDisplay::new(
            2,
            Rect::new(1440.0, 0.0, 1440.0, 900.0),
            2.0,
        ));

        // `screenN` → that display's full bounds.
        let mut a = ShotArgs::default();
        a.region = Some("screen1".to_string());
        let (r, i) = initial_region(&a, &s).expect("screen1 resolves");
        assert_eq!(i, 1);
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 1440.0, 900.0));

        // A global `WxH+X+Y` offsets into the display whose local bounds hold
        // its top-left (display 2's global origin is x=1440).
        a.region = Some("100x50+1500+10".to_string());
        let (r, i) = initial_region(&a, &s).expect("global rect resolves");
        assert_eq!(i, 1);
        assert_eq!((r.x, r.y, r.w, r.h), (60.0, 10.0, 100.0, 50.0));

        // On the primary the local coords equal the global ones.
        a.region = Some("100x50+10+20".to_string());
        let (r, i) = initial_region(&a, &s).expect("primary rect resolves");
        assert_eq!(i, 0);
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 20.0, 100.0, 50.0));

        // An out-of-range `screenN` and a non-rect fall through to `--last-region`.
        a.region = Some("screen9".to_string());
        a.last_region = true;
        s.state.last_region = Some(crate::engines::screenshot_annotations::ShotRegion {
            display: 2,
            x: 5.0,
            y: 6.0,
            w: 7.0,
            h: 8.0,
        });
        let (r, i) = initial_region(&a, &s).expect("last-region resolves");
        assert_eq!(i, 1);
        assert_eq!((r.x, r.y, r.w, r.h), (5.0, 6.0, 7.0, 8.0));

        // No region args → nothing preselected.
        let none = ShotArgs::default();
        assert!(initial_region(&none, &s).is_none());
    }

    #[test]
    fn delay_pending_blocks_idle_and_duplicate_triggers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingCapture(Arc<AtomicUsize>);
        impl ScreenCapture for CountingCapture {
            fn shareable_content(&self) -> Result<(), CaptureError> {
                Ok(())
            }
            fn capture(&self, _screens: &[ScreenRef]) -> Result<Vec<CapturedImage>, CaptureError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            }
        }

        let mut c = ScreenshotController::new();
        let n = Arc::new(AtomicUsize::new(0));
        let cap = CountingCapture(n.clone());

        // An outstanding delay timer keeps the controller busy and blocks a
        // second trigger (Swift leaves `capturing == true` across `asyncAfter`).
        c.delay_pending = true;
        assert!(!c.is_idle(), "a pending delay is not idle");
        assert!(c.trigger_with(&ShotArgs::default(), &cap).is_ok());
        assert_eq!(n.load(Ordering::SeqCst), 0, "no capture while a delay is pending");
        assert!(c.session.is_none());

        // Once the timer fires, the controller is idle again.
        c.delay_pending = false;
        assert!(c.is_idle());
    }

    #[test]
    fn rectangle_auto_selects_and_resizes_via_handles_with_undo() {
        let mut s = session();
        s.test_select(Rect::new(50.0, 50.0, 600.0, 400.0), Some(0));
        // Press R → Rectangle outline (`ShotTool::Selection`).
        assert!(s.handle_key(&shot_key_event("r").unwrap()));
        assert_eq!(s.tool, Some(ShotTool::Selection));

        // Draw rectangle from (100,100) to (220,180).
        s.mouse_down(0, Point::new(100.0, 100.0), 1, false, false);
        s.mouse_dragged(0, Point::new(220.0, 180.0), false, false);
        s.mouse_up(0, Point::new(220.0, 180.0), false, false);
        assert_eq!(s.doc.objects.len(), 1);
        assert_eq!(s.doc.selected, Some(0), "just-drawn rectangle is auto-selected for instant resize");
        assert_eq!(s.doc.objects[0].rect(), Rect::new(100.0, 100.0, 120.0, 80.0));

        // 8 handles exist on the selected rectangle; grab bottom-right corner (index 4).
        let handles = object_handles(&s.doc.objects[0]);
        assert_eq!(handles.len(), 8);
        assert_eq!(object_handle_at(&s.doc.objects[0], Point::new(223.0, 182.0)), Some(4));

        // Drag bottom-right corner +50x, +40y → 170x120.
        s.mouse_down(0, Point::new(220.0, 180.0), 1, false, false);
        s.mouse_dragged(0, Point::new(270.0, 220.0), false, false);
        s.mouse_up(0, Point::new(270.0, 220.0), false, false);
        assert_eq!(s.doc.objects[0].rect(), Rect::new(100.0, 100.0, 170.0, 120.0));
        assert_eq!(s.doc.selected, Some(0), "stays selected after resize");

        // Grab left mid-edge handle (index 7) and pull left by 20px → x=80, w=190.
        s.mouse_down(0, Point::new(100.0, 160.0), 1, false, false);
        s.mouse_dragged(0, Point::new(80.0, 160.0), false, false);
        s.mouse_up(0, Point::new(80.0, 160.0), false, false);
        assert_eq!(s.doc.objects[0].rect(), Rect::new(80.0, 100.0, 190.0, 120.0));

        // Undo reverts the edge resize cleanly in one step, then the corner resize.
        s.undo();
        assert_eq!(s.doc.objects[0].rect(), Rect::new(100.0, 100.0, 170.0, 120.0));
        s.undo();
        assert_eq!(s.doc.objects[0].rect(), Rect::new(100.0, 100.0, 120.0, 80.0));

        // Clicking near the thin outline (within forgiving slop) re-selects it.
        s.doc.selected = None;
        s.mouse_down(0, Point::new(106.0, 140.0), 1, false, false);
        s.mouse_up(0, Point::new(106.0, 140.0), false, false);
        assert_eq!(s.doc.selected, Some(0));
    }

    #[test]
    fn quick_bar_styles_live_text_and_selected_rectangle() {
        let mut cfg = ScreenshotConfig::default();
        cfg.user_colors = vec![
            None,
            ShotColor::from_hex("#ff0000"),
            ShotColor::from_hex("#ffff00"),
            ShotColor::from_hex("#0000ff"),
        ];
        let mut s = ShotSession::new(cfg, ShotArgs::default(), "/tmp/ks-shot-qb-test.json");
        s.add_display(ShotDisplay::new(1, Rect::new(0.0, 0.0, 1440.0, 900.0), 2.0));
        s.test_select(Rect::new(200.0, 200.0, 600.0, 400.0), Some(0));

        // 1. Text tool + live typing: quick-bar shows swatches, size, font, B, I.
        assert!(s.handle_key(&shot_key_event("t").unwrap()));
        s.begin_text(0, Point::new(300.0, 300.0), None);
        s.handle_key(&shot_key_event("h").unwrap());
        s.handle_key(&shot_key_event("i").unwrap());
        assert_eq!(s.editing.as_ref().unwrap().text, "hi");

        let qb = s.quick_bar_layout(0).expect("quick bar shown while typing text");
        assert!(qb.font.is_some() && qb.bold.is_some() && qb.italic.is_some());

        // Click blue swatch (index 2): updates live text color WITHOUT closing text edit!
        let blue_pt = Point::new(qb.swatches[2].0.mid_x(), qb.swatches[2].0.mid_y());
        assert!(s.quick_bar_click(blue_pt));
        assert!(s.editing.is_some(), "clicking quick bar keeps text draft open");
        assert_eq!(s.editing.as_ref().unwrap().object.color, ShotColor::from_hex("#0000ff").unwrap());

        // Click SizeUp (+), FontCycle, Bold, Italic on quick bar.
        let s0 = s.editing.as_ref().unwrap().object.size;
        assert!(s.quick_bar_click(Point::new(qb.size_up.mid_x(), qb.size_up.mid_y())));
        assert_eq!(s.editing.as_ref().unwrap().object.size, s0 + 2);
        assert!(s.quick_bar_click(Point::new(qb.font.unwrap().mid_x(), qb.font.unwrap().mid_y())));
        assert_eq!(s.editing.as_ref().unwrap().object.style.family, "Menlo");
        assert!(s.quick_bar_click(Point::new(qb.bold.unwrap().mid_x(), qb.bold.unwrap().mid_y())));
        assert!(s.editing.as_ref().unwrap().object.style.bold);
        assert!(s.quick_bar_click(Point::new(qb.italic.unwrap().mid_x(), qb.italic.unwrap().mid_y())));
        assert!(s.editing.as_ref().unwrap().object.style.italic);

        // Live keyboard shortcuts while editing text: Cmd+B, Cmd+=, Cmd+-.
        assert!(s.handle_key(&shot_key_event("cmd+b").unwrap()));
        assert!(!s.editing.as_ref().unwrap().object.style.bold);
        let s1 = s.editing.as_ref().unwrap().object.size;
        assert!(s.handle_key(&KeyInput {
            key_code: 24, chars: Some('='), cmd: true, ctrl: false, shift: false, opt: false, esc_streak: 0,
        }));
        assert_eq!(s.editing.as_ref().unwrap().object.size, s1 + 2);
        s.commit_text();

        // 2. Selected Rectangle (R): quick bar hides font/B/I, recolors & thickens live.
        assert!(s.handle_key(&shot_key_event("r").unwrap()));
        s.mouse_down(0, Point::new(250.0, 250.0), 1, false, false);
        s.mouse_dragged(0, Point::new(400.0, 350.0), false, false);
        s.mouse_up(0, Point::new(400.0, 350.0), false, false);
        let idx = s.doc.selected.expect("rectangle selected");
        let rqb = s.quick_bar_layout(0).expect("quick bar shown for rectangle");
        assert!(rqb.font.is_none(), "non-text tool omits font controls");

        // Click red swatch + size up + keyboard ']' shortcut.
        assert!(s.quick_bar_click(Point::new(rqb.swatches[0].0.mid_x(), rqb.swatches[0].0.mid_y())));
        assert_eq!(s.doc.objects[idx].color, ShotColor::from_hex("#ff0000").unwrap());
        let w0 = s.doc.objects[idx].size;
        assert!(s.quick_bar_click(Point::new(rqb.size_up.mid_x(), rqb.size_up.mid_y())));
        assert_eq!(s.doc.objects[idx].size, w0 + 1);
        assert!(s.handle_key(&KeyInput {
            key_code: 30, chars: Some(']'), cmd: false, ctrl: false, shift: false, opt: false, esc_streak: 0,
        }));
        assert_eq!(s.doc.objects[idx].size, w0 + 2);
        let _ = std::fs::remove_file("/tmp/ks-shot-qb-test.json");
    }

    #[test]
    fn highlighter_freehand_shift_snap_and_dedicated_color() {
        let mut s = session();
        s.test_select(Rect::new(50.0, 50.0, 600.0, 400.0), Some(0));

        // Both H and M select Highlighter (`ShotTool::Marker`), defaulting to bright yellow.
        assert_eq!(ShotTool::for_letter('h'), Some(ShotTool::Marker));
        assert_eq!(ShotTool::for_letter('m'), Some(ShotTool::Marker));
        assert!(s.handle_key(&shot_key_event("h").unwrap()));
        assert_eq!(s.tool, Some(ShotTool::Marker));
        assert_eq!(s.current_color(), ShotColor::from_hex("#ffe600").unwrap());

        // Freehand drag appends multiple smooth points.
        s.mouse_down(0, Point::new(100.0, 120.0), 1, false, false);
        s.mouse_dragged(0, Point::new(130.0, 122.0), false, false);
        s.mouse_dragged(0, Point::new(170.0, 125.0), false, false);
        s.mouse_dragged(0, Point::new(210.0, 120.0), false, false);
        s.mouse_up(0, Point::new(210.0, 120.0), false, false);
        assert_eq!(s.doc.objects.len(), 1);
        assert_eq!(s.doc.objects[0].tool, ShotTool::Marker);
        assert_eq!(s.doc.objects[0].color, ShotColor::from_hex("#ffe600").unwrap());
        assert!(s.doc.objects[0].points.len() >= 4, "freehand highlighter captures path");

        // Shift-drag snaps to a straight horizontal highlight bar (2 points).
        s.mouse_down(0, Point::new(100.0, 200.0), 1, false, false);
        s.mouse_dragged(0, Point::new(300.0, 206.0), true, false);
        s.mouse_up(0, Point::new(300.0, 206.0), true, false);
        assert_eq!(s.doc.objects.len(), 2);
        assert_eq!(s.doc.objects[1].points.len(), 2);
        assert!((s.doc.objects[1].end().y - 200.0).abs() < 0.01, "Shift snaps straight line");

        // Changing highlighter color keeps rectangle/pen color independent!
        let pen_before = s.color;
        let green = ShotColor::from_hex("#00ff00").unwrap();
        s.set_color(green);
        assert_eq!(s.marker_color, green);
        assert_eq!(s.color, pen_before, "regular draw color unchanged");
    }

    #[test]
    fn help_opacity_config_and_abort_clears_pending_capture_and_delay() {
        let mut map = HashMap::new();
        map.insert("help-opacity".to_string(), "0.35".to_string());
        map.insert("highlighter-color".to_string(), "#ffaa00".to_string());
        let cfg = ScreenshotConfig::from_entries(&map);
        assert!((cfg.help_opacity - 0.35).abs() < 1e-6);
        assert_eq!(cfg.highlighter_color, ShotColor::from_hex("#ffaa00").unwrap());

        let mut c = ScreenshotController::new();
        c.capture_pending = true;
        c.delay_pending = true;
        let gen0 = c.capture_gen;
        let dgen0 = c.delay_gen;
        assert!(!c.is_idle());
        c.abort_session();
        assert!(c.is_idle(), "abort clears pending capture and delay");
        assert_ne!(c.capture_gen, gen0, "capture_gen bumped so late capture drops");
        assert_ne!(c.delay_gen, dgen0, "delay_gen bumped so stale delay timer drops");
    }
}
