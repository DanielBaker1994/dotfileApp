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
}
