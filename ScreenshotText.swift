import Foundation
import CoreGraphics
import CoreText
import Vision

// /screenshot's Copy Text mode (Tab / O in the overlay, ⇧⌘C, the ring's
// copy-text button): the selection's pixels → Apple Vision's on-device text
// recognizer → one string for the clipboard. No AppKit: bin/run-tests.sh
// screenshot renders text and reads it back.

struct ShotOCRConfig {
    var languages: [String] = []      // empty = Vision detects the language
    var correction = true             // Vision's language correction
}

// one recognized line, box in image pixels, top-left origin
struct ShotOCRLine: Equatable {
    var text: String
    var box: CGRect
}

enum ShotOCR {
    // Vision misses very small text: crops shorter than this are upscaled 2×
    static let minHeight = 64

    static func recognize(_ img: CGImage, _ cfg: ShotOCRConfig = ShotOCRConfig()) -> [ShotOCRLine] {
        let scale = img.height < minHeight ? 2 : 1
        let input = scale > 1 ? (upscaled(img, scale) ?? img) : img
        let req = VNRecognizeTextRequest()
        req.recognitionLevel = .accurate
        req.usesLanguageCorrection = cfg.correction
        if cfg.languages.isEmpty { req.automaticallyDetectsLanguage = true }
        else { req.recognitionLanguages = cfg.languages }
        do { try VNImageRequestHandler(cgImage: input, options: [:]).perform([req]) } catch { return [] }
        let w = CGFloat(img.width), h = CGFloat(img.height)
        return (req.results ?? []).compactMap { o -> ShotOCRLine? in
            guard let t = o.topCandidates(1).first?.string, !t.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
            // Vision: normalized, bottom-left origin
            let b = o.boundingBox
            return ShotOCRLine(text: t, box: CGRect(x: b.minX * w, y: (1 - b.maxY) * h, width: b.width * w, height: b.height * h))
        }
    }

    // reading order: rows top → bottom (lines whose vertical centers sit
    // inside one another's band are one row, left → right, joined by a
    // space), a blank line where the gap between rows is taller than a line
    static func join(_ lines: [ShotOCRLine]) -> String {
        var rows: [[ShotOCRLine]] = []
        for l in lines.sorted(by: { $0.box.midY < $1.box.midY }) {
            if let i = rows.indices.last, let band = rows[i].first?.box,
               (band.minY...band.maxY).contains(l.box.midY) || (l.box.minY...l.box.maxY).contains(band.midY) {
                rows[i].append(l)
            } else {
                rows.append([l])
            }
        }
        var out = ""
        var prev: CGRect?
        for r in rows {
            let row = r.sorted { $0.box.minX < $1.box.minX }
            let box = row.reduce(CGRect.null) { $0.union($1.box) }
            if let p = prev {
                out += "\n"
                let lh = min(p.height, box.height)
                if box.minY - p.maxY > lh { out += "\n" }
            }
            out += row.map { $0.text.trimmingCharacters(in: .whitespaces) }.joined(separator: " ")
            prev = box
        }
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func text(_ img: CGImage, _ cfg: ShotOCRConfig = ShotOCRConfig()) -> String {
        join(recognize(img, cfg))
    }

    // The first recognition in a process makes the Neural Engine compile
    // the model (≈ 50 s seen on the owner's Mac, then ≈ 0.2 s): run it once
    // on a few rendered words at launch, off main. Returns the ms it took.
    static func warmUp(_ cfg: ShotOCRConfig = ShotOCRConfig()) -> Int {
        let t0 = DispatchTime.now().uptimeNanoseconds
        let w = 240, h = 48
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return 0 }
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        let font = CTFontCreateWithName("Helvetica" as CFString, 24, nil)
        let s = NSAttributedString(string: "Warm up text", attributes: [
            kCTFontAttributeName as NSAttributedString.Key: font,
            kCTForegroundColorAttributeName as NSAttributedString.Key: CGColor(red: 0, green: 0, blue: 0, alpha: 1)])
        ctx.textPosition = CGPoint(x: 8, y: 14)
        CTLineDraw(CTLineCreateWithAttributedString(s), ctx)
        if let img = ctx.makeImage() { _ = recognize(img, cfg) }
        return Int(Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000)
    }

    // the toast's {}: "3 lines" / "42 characters"
    static func summary(_ s: String) -> String {
        let n = s.split(separator: "\n", omittingEmptySubsequences: true).count
        return n > 1 ? "\(n) lines" : "\(s.count) characters"
    }

    private static func upscaled(_ img: CGImage, _ k: Int) -> CGImage? {
        guard let ctx = CGContext(data: nil, width: img.width * k, height: img.height * k, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .high
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: img.width * k, height: img.height * k))
        return ctx.makeImage()
    }
}
