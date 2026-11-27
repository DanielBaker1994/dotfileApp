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
    /// Vision's `.accurate` level instead of `.fast` (`[screenshot]
    /// text-accuracy`). On macOS 27 (26A428) `.accurate` is broken: its
    /// precompiled model fails to load (E5RT code 13) on ANE, GPU and CPU
    /// alike, ~43 s per process and then an error. `.fast` reads screen text
    /// in ~100–200 ms.
    pub accurate: bool,
}

impl Default for ShotOCRConfig {
    fn default() -> Self {
        ShotOCRConfig {
            languages: Vec::new(),
            correction: true,
            accurate: false,
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

    /// Images up to this many pixels are doubled for `.fast` recognition.
    pub const FAST_UPSCALE_MAX_PIXELS: i64 = 6_000_000;

    /// The scale to recognise at. `.fast` misreads screen text at native
    /// size: on a Retina capture of a monospace table it split "Owner" into
    /// "Owne r" and read "2026_11_27" as "2026 11 27"; at 2× both came out
    /// right, for +20 ms on a crop and +35 ms on a whole 2048×1536 screen.
    /// So `.fast` doubles anything up to [`Self::FAST_UPSCALE_MAX_PIXELS`];
    /// `.accurate` keeps the Swift rule.
    pub fn scale_for(width: i64, height: i64, accurate: bool) -> i64 {
        if height < Self::MIN_HEIGHT {
            return 2;
        }
        if !accurate && width.saturating_mul(height) <= Self::FAST_UPSCALE_MAX_PIXELS {
            return 2;
        }
        1
    }

    /// Undo the space `.fast` puts before a file extension in a path:
    /// "/x/daemon. log" → "/x/daemon.log". Only when the word before the dot
    /// holds a `/` and the word after is a short lowercase extension, so
    /// prose ("End. Next", "and/or. Then") is left alone.
    pub fn tidy_paths(text: &str) -> String {
        text.split('\n')
            .map(|line| {
                let mut out = String::with_capacity(line.len());
                let mut rest = line;
                while let Some(i) = rest.find(". ") {
                    let (head, tail) = rest.split_at(i + 1);
                    let word_start = head.rfind([' ', '\t']).map(|j| j + 1).unwrap_or(0);
                    let word = &head[word_start..];
                    let ext_len = tail[1..]
                        .chars()
                        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                        .count();
                    let ext_end = tail[1..].chars().nth(ext_len);
                    let ext_ok = (1..=5).contains(&ext_len)
                        && ext_end.is_none_or(|c| c.is_whitespace() || ",;:)]'\"".contains(c));
                    out.push_str(head);
                    if word.contains('/') && word.len() > 2 && ext_ok {
                        rest = &tail[1..];
                    } else {
                        rest = tail;
                    }
                }
                out.push_str(rest);
                out
            })
            .collect::<Vec<_>>()
            .join("\n")
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
        let scale = Self::scale_for(
            CGImage::width(Some(img)) as i64,
            CGImage::height(Some(img)) as i64,
            cfg.accurate,
        ) as usize;
        let upscaled_owner = if scale > 1 {
            upscaled(img, scale)
        } else {
            None
        };
        let input: &CGImage = upscaled_owner.as_deref().unwrap_or(img);

        let req = VNRecognizeTextRequest::new();
        req.setRecognitionLevel(if cfg.accurate {
            VNRequestTextRecognitionLevel::Accurate
        } else {
            VNRequestTextRecognitionLevel::Fast
        });
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

    /// Every QR / barcode payload in `img`, top to bottom (Vision's barcode
    /// request; empty when there is none or it fails).
    #[cfg(target_os = "macos")]
    pub fn barcodes(img: &CGImage) -> Vec<String> {
        use objc2_vision::VNDetectBarcodesRequest;
        let req = unsafe { VNDetectBarcodesRequest::new() };
        let options = NSDictionary::dictionary();
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), img, &options)
        };
        let req_ref: &VNRequest = unsafe { &*(Retained::as_ptr(&req) as *const VNRequest) };
        if handler.performRequests_error(&NSArray::arrayWithObject(req_ref)).is_err() {
            return Vec::new();
        }
        let mut found: Vec<(f64, String)> = Vec::new();
        if let Some(results) = unsafe { req.results() } {
            for o in results.iter() {
                let Some(p) = (unsafe { o.payloadStringValue() }) else { continue };
                let p = p.to_string();
                if p.trim().is_empty() || found.iter().any(|(_, q)| *q == p) {
                    continue;
                }
                // Vision's origin is bottom-left: a higher y is nearer the top.
                let y = unsafe { o.boundingBox() }.origin.y;
                found.push((-y, p));
            }
        }
        found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        found.into_iter().map(|(_, p)| p).collect()
    }

    pub fn join(lines: &[ShotOCRLine]) -> String {
        ShotOCR::join_columns(lines, 0.0)
    }

    /// [`ShotOCR::join`], but two blocks on one row whose horizontal gap is
    /// wider than `column_gap` × the line height are joined with a tab
    /// instead of a space — table columns survive the copy (`[screenshot]
    /// text-column-gap`; `0` = always a space, the Swift behavior).
    pub fn join_columns(lines: &[ShotOCRLine], column_gap: f64) -> String {
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
            let mut prev_cell: Option<Rect> = None;
            for l in &row {
                if let Some(p) = prev_cell {
                    let gap = l.bounding_box.min_x() - p.max_x();
                    let lh = p.h.min(l.bounding_box.h);
                    let tab = column_gap > 0.0 && lh > 0.0 && gap > column_gap * lh;
                    out.push(if tab { '\t' } else { ' ' });
                }
                out.push_str(l.text.trim());
                prev_cell = Some(l.bounding_box);
            }
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
    /// `0` when the bitmap context cannot be created. CoreGraphics, CoreText
    /// and Vision only, so any thread; launch prewarms use
    /// [`Self::warm_up_in_background`].
    #[cfg(target_os = "macos")]
    pub fn warm_up_in_background(cfg: ShotOCRConfig) {
        // Never on the main thread: a cold Vision load compiles the text model
        // for the Neural Engine and can take many seconds — on main it froze
        // the freshly launched window (2026-11-25 stall sample).
        let _ = std::thread::Builder::new().name("ks-ocr-warmup".into()).spawn(move || {
            let ms = ShotOCR::warm_up(&cfg);
            crate::app::logging::ws_log(&format!("screenshot: OCR warm-up {ms} ms (background)"));
        });
    }

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
    fn join_columns_tabs_wide_gaps_only() {
        // Boxes are 100 wide, 20 tall: x=110 is a 10 pt gap, x=300 a 100 pt one.
        let row = [line("Name", 0.0, 0.0), line("In", 110.0, 0.0), line("Owner", 300.0, 0.0)];
        assert_eq!(ShotOCR::join_columns(&row, 1.5), "Name In\tOwner", "only the wide gap tabs");
        assert_eq!(ShotOCR::join_columns(&row, 0.0), "Name In Owner", "0 = the old join");
        assert_eq!(ShotOCR::join(&row), ShotOCR::join_columns(&row, 0.0));
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
    fn fast_ocr_doubles_screen_sized_images() {
        assert_eq!(ShotOCR::scale_for(784, 170, false), 2, "a crop: doubled");
        assert_eq!(ShotOCR::scale_for(2048, 1536, false), 2, "a Retina laptop screen: doubled");
        assert_eq!(ShotOCR::scale_for(6016, 3384, false), 1, "a 6K screen: native");
        assert_eq!(ShotOCR::scale_for(784, 170, true), 1, "accurate keeps the Swift rule");
        assert_eq!(ShotOCR::scale_for(784, 40, true), 2, "short images always doubled");
    }

    #[test]
    fn tidy_paths_rejoins_split_extensions() {
        assert_eq!(
            ShotOCR::tidy_paths("Error: ENOENT /Users/a/logs/daemon. log"),
            "Error: ENOENT /Users/a/logs/daemon.log"
        );
        assert_eq!(ShotOCR::tidy_paths("see ~/x/a. md, then"), "see ~/x/a.md, then");
        assert_eq!(ShotOCR::tidy_paths("End. Next line"), "End. Next line", "prose untouched");
        assert_eq!(ShotOCR::tidy_paths("and/or. Then"), "and/or. Then", "long next word is prose");
        assert_eq!(ShotOCR::tidy_paths("a/b. c\nd/e. f"), "a/b.c\nd/e.f", "per line");
        assert_eq!(ShotOCR::tidy_paths("tail /x/y. "), "tail /x/y. ", "nothing after the space");
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
