//! Port of `ScreenshotText.swift` — the copy-text (OCR) model.
//!
//! `ShotOCR::join` / `summary` are pure; `recognize` drives Vision
//! (`VNRecognizeTextRequest`, `.accurate`) over a `CGImage`, applying the
//! Swift crop-upscale rule (a capture under `MIN_HEIGHT` is doubled first).

use super::screenshot_annotations::Rect;

#[cfg(target_os = "macos")]
use core::ptr;
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::AnyThread;
#[cfg(target_os = "macos")]
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext,
    CGImage, CGImageAlphaInfo, CGInterpolationQuality,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString};
#[cfg(target_os = "macos")]
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ShotOCRConfig {
    pub languages: Vec<String>,
    pub correction: bool,
}

impl Default for ShotOCRConfig {
    fn default() -> Self {
        ShotOCRConfig {
            languages: Vec::new(),
            correction: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotOCRLine {
    pub text: String,
    pub bounding_box: Rect,
}

impl ShotOCRLine {
    pub fn new(text: &str, bounding_box: Rect) -> Self {
        ShotOCRLine {
            text: text.to_string(),
            bounding_box,
        }
    }
}

pub struct ShotOCR;

impl ShotOCR {
    pub const MIN_HEIGHT: i64 = 64;

    /// Swift's crop-upscale rule: an image shorter than `MIN_HEIGHT` is
    /// upscaled 2× before recognition.
    pub fn upscale_factor(height: i64) -> i64 {
        if height < Self::MIN_HEIGHT {
            2
        } else {
            1
        }
    }

    /// Vision boxes are normalized with a bottom-left origin; `join` wants
    /// pixel coordinates with a top-left origin.
    pub fn map_box(
        min_x: f64,
        min_y: f64,
        width: f64,
        height: f64,
        img_w: f64,
        img_h: f64,
    ) -> Rect {
        Rect::new(
            min_x * img_w,
            (1.0 - (min_y + height)) * img_h,
            width * img_w,
            height * img_h,
        )
    }

    #[cfg(target_os = "macos")]
    pub fn recognize(img: &CGImage, cfg: &ShotOCRConfig) -> Vec<ShotOCRLine> {
        let scale = Self::upscale_factor(CGImage::height(Some(img)) as i64) as usize;
        let upscaled_owner = if scale > 1 {
            upscaled(img, scale)
        } else {
            None
        };
        let input: &CGImage = upscaled_owner.as_deref().unwrap_or(img);

        let req = VNRecognizeTextRequest::new();
        req.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        req.setUsesLanguageCorrection(cfg.correction);
        if cfg.languages.is_empty() {
            req.setAutomaticallyDetectsLanguage(true);
        } else {
            let langs: Vec<Retained<NSString>> =
                cfg.languages.iter().map(|s| NSString::from_str(s)).collect();
            req.setRecognitionLanguages(&NSArray::from_retained_slice(&langs));
        }

        let options = NSDictionary::dictionary();
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                input,
                &options,
            )
        };
        let req_ref: &VNRequest = unsafe { &*(Retained::as_ptr(&req) as *const VNRequest) };
        if handler.performRequests_error(&NSArray::arrayWithObject(req_ref)).is_err() {
            return Vec::new();
        }

        let w = CGImage::width(Some(img)) as f64;
        let h = CGImage::height(Some(img)) as f64;
        let mut out = Vec::new();
        if let Some(results) = req.results() {
            for o in results.iter() {
                let Some(top) = o.topCandidates(1).firstObject() else {
                    continue;
                };
                let text = top.string().to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let b = unsafe { o.boundingBox() };
                out.push(ShotOCRLine::new(
                    &text,
                    Self::map_box(b.origin.x, b.origin.y, b.size.width, b.size.height, w, h),
                ));
            }
        }
        out
    }

    /// Non-macOS OCR (W6-D): Vision is macOS-only, so this twin takes the
    /// capture's **PNG bytes** (macOS decodes them to a `CGImage` first) and
    /// shells out to the `tesseract` CLI. `None` when `tesseract` is absent,
    /// the bytes are not a PNG, or the command fails — the caller reads that
    /// as "no text", like the macOS twin's empty `Vec`. Plain-text stdout has
    /// no boxes, so each non-empty line gets a full-image-width box stacked
    /// top→bottom, preserving order through [`ShotOCR::join`].
    #[cfg(not(target_os = "macos"))]
    pub fn recognize(png: &[u8], cfg: &ShotOCRConfig) -> Option<Vec<ShotOCRLine>> {
        let (img_w, img_h) = png_size(png)?;
        if !tesseract_available() {
            return None;
        }
        let path = write_temp_png(png)?;
        let mut cmd = std::process::Command::new("tesseract");
        cmd.arg(&path).arg("stdout");
        if let Some(langs) = tesseract_langs(&cfg.languages) {
            cmd.arg("-l").arg(langs);
        }
        let output = cmd.output();
        let _ = std::fs::remove_file(&path);
        let output = output.ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Some(parse_tesseract_stdout(&stdout, img_w, img_h))
    }

    pub fn join(lines: &[ShotOCRLine]) -> String {
        let mut sorted = lines.to_vec();
        sorted.sort_by(|a, b| {
            a.bounding_box
                .mid_y()
                .partial_cmp(&b.bounding_box.mid_y())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut rows: Vec<Vec<ShotOCRLine>> = Vec::new();
        for l in sorted {
            let mut placed = false;
            if let Some(i) = rows.len().checked_sub(1) {
                let band = rows[i][0].bounding_box;
                if (band.min_y()..=band.max_y()).contains(&l.bounding_box.mid_y())
                    || (l.bounding_box.min_y()..=l.bounding_box.max_y()).contains(&band.mid_y())
                {
                    rows[i].push(l.clone());
                    placed = true;
                }
            }
            if !placed {
                rows.push(vec![l]);
            }
        }

        let mut out = String::new();
        let mut prev: Option<Rect> = None;
        for r in rows {
            let mut row = r;
            row.sort_by(|a, b| {
                a.bounding_box
                    .min_x()
                    .partial_cmp(&b.bounding_box.min_x())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut b = row[0].bounding_box;
            for l in row.iter().skip(1) {
                b = b.union(&l.bounding_box);
            }
            if let Some(p) = prev {
                out.push('\n');
                let lh = p.h.min(b.h);
                if b.min_y() - p.max_y() > lh {
                    out.push('\n');
                }
            }
            let joined = row
                .iter()
                .map(|l| l.text.trim().to_string())
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&joined);
            prev = Some(b);
        }
        out.trim().to_string()
    }

    pub fn text(lines: &[ShotOCRLine]) -> String {
        ShotOCR::join(lines)
    }

    /// `String(text.prefix(max))` — keep at most `max` Unicode scalars. The
    /// `lastOutput["text"]` field is capped at 4000 so a giant capture cannot
    /// balloon the socket `state` JSON.
    pub fn truncate(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else {
            s.chars().take(max).collect()
        }
    }

    pub fn summary(s: &str) -> String {
        let n = s.split('\n').filter(|x| !x.is_empty()).count();
        if n > 1 {
            format!("{} lines", n)
        } else {
            format!("{} characters", s.chars().count())
        }
    }

    /// `ShotOCR.warmUp`: draw a small "Warm up text" bitmap with CoreText and
    /// run a recognition pass over it, returning the elapsed milliseconds.
    /// `0` when the bitmap context cannot be created. Main-thread callers only
    /// (see [`crate::views::screenshot::ScreenshotController::prewarm`]).
    #[cfg(target_os = "macos")]
    pub fn warm_up(cfg: &ShotOCRConfig) -> i64 {
        use objc2::runtime::AnyObject;
        use objc2_core_graphics::CGColor;
        use objc2_core_text::{
            kCTFontAttributeName, kCTForegroundColorAttributeName, CTFont, CTLine,
        };
        use objc2_foundation::{NSMutableAttributedString, NSRange};

        let start = std::time::Instant::now();
        let (w, h) = (240i64, 48i64);
        let Some(space) = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })) else {
            return 0;
        };
        let Some(ctx) = (unsafe {
            CGBitmapContextCreate(
                ptr::null_mut(),
                w as usize,
                h as usize,
                8,
                0,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }) else {
            return 0;
        };
        let white = CGColor::new_srgb(1.0, 1.0, 1.0, 1.0);
        CGContext::set_fill_color_with_color(Some(&*ctx), Some(&*white));
        CGContext::fill_rect(
            Some(&*ctx),
            NSRect::new(NSPoint::ZERO, NSSize::new(w as f64, h as f64)),
        );
        let name = NSString::from_str("Helvetica");
        let font: Retained<CTFont> =
            unsafe { CTFont::with_name(ffi_cast(&*name), 24.0, ptr::null()).into() };
        let text = NSString::from_str("Warm up text");
        let attr =
            NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &text);
        let len = text.len_utf16();
        let font_obj: &AnyObject = ffi_cast(&*font);
        unsafe {
            attr.addAttribute_value_range(
                ffi_cast::<_, NSString>(kCTFontAttributeName),
                font_obj,
                NSRange::new(0, len),
            );
        }
        let black = CGColor::new_srgb(0.0, 0.0, 0.0, 1.0);
        let color_obj: &AnyObject = ffi_cast(&*black);
        unsafe {
            attr.addAttribute_value_range(
                ffi_cast::<_, NSString>(kCTForegroundColorAttributeName),
                color_obj,
                NSRange::new(0, len),
            );
        }
        let line: Retained<CTLine> =
            unsafe { CTLine::with_attributed_string(ffi_cast(&*attr)).into() };
        CGContext::set_text_position(Some(&*ctx), 8.0, 14.0);
        unsafe { line.draw(&ctx) };
        if let Some(img) = CGBitmapContextCreateImage(Some(&*ctx)) {
            let _ = Self::recognize(&img, cfg);
        }
        start.elapsed().as_millis() as i64
    }
}

/// Toll-free reinterpret one CoreFoundation reference as another (mirrors the
/// `ffi_cast` helper in `views/screenshot.rs`).
#[cfg(target_os = "macos")]
fn ffi_cast<T, U>(r: &T) -> &U {
    unsafe { &*(r as *const T as *const U) }
}

#[cfg(target_os = "macos")]
fn upscaled(img: &CGImage, k: usize) -> Option<impl std::ops::Deref<Target = CGImage>> {
    let iw = CGImage::width(Some(img));
    let ih = CGImage::height(Some(img));
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let ctx = unsafe {
        CGBitmapContextCreate(
            ptr::null_mut(),
            iw * k,
            ih * k,
            8,
            0,
            Some(&*space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    CGContext::set_interpolation_quality(Some(&*ctx), CGInterpolationQuality::High);
    CGContext::draw_image(
        Some(&*ctx),
        NSRect::new(NSPoint::ZERO, NSSize::new((iw * k) as f64, (ih * k) as f64)),
        Some(img),
    );
    CGBitmapContextCreateImage(Some(&*ctx))
}

/// PNG width/height from the `IHDR` chunk (be u32s at bytes 16..24); `None`
/// when the bytes are not a PNG. Pure std (W6-D).
fn png_size(png: &[u8]) -> Option<(f64, f64)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 24 || !png.starts_with(&SIG) || png[12..16] != b"IHDR"[..] {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as f64;
    Some((be(16), be(20)))
}

/// Parse `tesseract <image> stdout` output. The plain-text mode emits no
/// bounding boxes, so each non-empty line is boxed full-width (`img_w`) and
/// stacked to fill `img_h` (one equal band per line) — enough for
/// [`ShotOCR::join`] to keep tesseract's top→bottom reading order.
fn parse_tesseract_stdout(stdout: &str, img_w: f64, img_h: f64) -> Vec<ShotOCRLine> {
    let texts: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if texts.is_empty() {
        return Vec::new();
    }
    let w = if img_w > 0.0 { img_w } else { 1.0 };
    let lh = if img_h > 0.0 { img_h / texts.len() as f64 } else { 1.0 };
    texts
        .into_iter()
        .enumerate()
        .map(|(i, t)| ShotOCRLine::new(t, Rect::new(0.0, i as f64 * lh, w, lh)))
        .collect()
}

/// Vision-style codes (`en-US`) → tesseract's (`en`), dropping region suffix
/// and duplicates; `None` when nothing usable is left (tesseract default).
fn tesseract_langs(languages: &[String]) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for l in languages {
        let b = l.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
        if !b.is_empty() && b.chars().all(|c| c.is_ascii_alphabetic()) && !out.contains(&b) {
            out.push(b);
        }
    }
    if out.is_empty() { None } else { Some(out.join("+")) }
}

/// `tesseract --version` succeeds ⇒ the CLI is on `PATH` (W6-D).
#[cfg(not(target_os = "macos"))]
fn tesseract_available() -> bool {
    std::process::Command::new("tesseract")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write the PNG to a unique temp file and return its path (W6-D).
#[cfg(not(target_os = "macos"))]
fn write_temp_png(png: &[u8]) -> Option<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("ks-ocr-{}-{}.png", std::process::id(), seq));
    std::fs::write(&path, png).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: &str, x: f64, y: f64) -> ShotOCRLine {
        ShotOCRLine::new(t, Rect::new(x, y, 100.0, 20.0))
    }

    #[test]
    fn join_empty_and_rows() {
        assert_eq!(ShotOCR::join(&[]), "", "nothing → empty");
        assert_eq!(
            ShotOCR::join(&[line("second", 0.0, 30.0), line("first", 0.0, 0.0)]),
            "first\nsecond",
            "rows top → bottom"
        );
        assert_eq!(
            ShotOCR::join(&[line("right", 200.0, 2.0), line("left", 0.0, 0.0)]),
            "left right",
            "one row, left → right"
        );
        assert_eq!(
            ShotOCR::join(&[line("a", 0.0, 0.0), line("b", 0.0, 25.0), line("c", 0.0, 90.0)]),
            "a\nb\n\nc",
            "a tall gap = a paragraph"
        );
        assert_eq!(ShotOCR::join(&[line("  pad  ", 0.0, 0.0)]), "pad", "trimmed");
    }

    #[test]
    fn summary_test() {
        assert_eq!(ShotOCR::summary("one line"), "8 characters");
        assert_eq!(ShotOCR::summary("a\nb\nc"), "3 lines");
    }

    #[test]
    fn truncate_caps_the_reported_text() {
        assert_eq!(ShotOCR::truncate("abc", 4000), "abc", "short text untouched");
        assert_eq!(ShotOCR::truncate("abc", 3), "abc", "exact length kept");
        assert_eq!(ShotOCR::truncate("abcdef", 3), "abc", "long text clipped");
        assert_eq!(ShotOCR::truncate("", 0), "", "empty stays empty");
    }

    #[test]
    fn upscale_rule() {
        assert_eq!(ShotOCR::upscale_factor(0), 2);
        assert_eq!(ShotOCR::upscale_factor(63), 2);
        assert_eq!(ShotOCR::upscale_factor(64), 1, "MIN_HEIGHT is not short");
        assert_eq!(ShotOCR::upscale_factor(1000), 1);
    }

    #[test]
    fn box_mapping() {
        let r = ShotOCR::map_box(0.25, 0.5, 0.5, 0.25, 400.0, 200.0);
        assert_eq!(r, Rect::new(100.0, 50.0, 200.0, 50.0), "bottom-left → top-left px");
        let full = ShotOCR::map_box(0.0, 0.0, 1.0, 1.0, 400.0, 200.0);
        assert_eq!(full, Rect::new(0.0, 0.0, 400.0, 200.0), "whole image");
    }

    #[test]
    fn tesseract_stdout_parsing_keeps_reading_order() {
        let lines = parse_tesseract_stdout("first\n  second  \n\nthird\n", 400.0, 60.0);
        assert_eq!(lines.len(), 3, "blank lines dropped");
        assert_eq!(lines[0].bounding_box, Rect::new(0.0, 0.0, 400.0, 20.0));
        assert_eq!(lines[1].bounding_box, Rect::new(0.0, 20.0, 400.0, 20.0));
        assert_eq!(lines[2].bounding_box, Rect::new(0.0, 40.0, 400.0, 20.0));
        assert_eq!(
            ShotOCR::text(&lines),
            "first\nsecond\nthird",
            "join preserves tesseract order"
        );
        assert!(parse_tesseract_stdout("", 10.0, 10.0).is_empty(), "empty → no lines");
        assert!(parse_tesseract_stdout("  \n \n", 10.0, 10.0).is_empty(), "blank → no lines");
    }

    #[test]
    fn png_size_reads_ihdr() {
        let mut png = vec![0u8; 24];
        png[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&640u32.to_be_bytes());
        png[20..24].copy_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((640.0, 480.0)));
        assert_eq!(png_size(b"not a png"), None, "bad magic");
        assert_eq!(png_size(&[]), None, "too short");
    }

    #[test]
    fn tesseract_languages_map_to_base_codes() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(tesseract_langs(&v(&["en-US", "de", "en"])).as_deref(), Some("en+de"));
        assert_eq!(tesseract_langs(&[]), None, "none → tesseract default");
        assert_eq!(tesseract_langs(&v(&["", "??"])), None, "unusable → default");
    }
}
