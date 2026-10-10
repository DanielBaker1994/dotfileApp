import CoreGraphics
import CoreText
import Foundation

struct AnsiRGB: Equatable {
    var r: UInt8, g: UInt8, b: UInt8

    init(_ r: UInt8, _ g: UInt8, _ b: UInt8) { self.r = r; self.g = g; self.b = b }

    init?(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        self.init(UInt8(v >> 16 & 0xff), UInt8(v >> 8 & 0xff), UInt8(v & 0xff))
    }
}

enum AnsiColor: Equatable {
    case none
    case index(Int)
    case rgb(AnsiRGB)
}

struct AnsiStyle: Equatable {
    var fg = AnsiColor.none, bg = AnsiColor.none
    var bold = false, dim = false, italic = false
    var underline = false, inverse = false, strike = false
}

struct AnsiCell: Equatable {
    var text: String
    var style: AnsiStyle
    var width: Int
}

struct AnsiGrid {
    var rows: [[AnsiCell]] = []

    var columns: Int { rows.map(\.count).max() ?? 0 }

    /// The ANSI parser lives in pylib/ansi.py (its suite's parser cases
    /// moved there); this decodes the cells into the renderer's mirror.
    static func parse(_ text: String) -> AnsiGrid {
        var g = AnsiGrid()
        guard case .success(let box) = PythonHelper.shared.callSync(
                "ansi.parse", ["text": text], timeout: 60),
              let d = box as? [String: Any], let rows = d["rows"] as? [[[String: Any]]] else {
            return g
        }
        g.rows = rows.map { row in
            row.map { c in
                AnsiCell(text: c["text"] as? String ?? "",
                         style: Self.style(from: c["style"] as? [String: Any] ?? [:]),
                         width: c["width"] as? Int ?? 1)
            }
        }
        return g
    }

    private static func style(from d: [String: Any]) -> AnsiStyle {
        func color(_ v: Any?) -> AnsiColor {
            guard let a = v as? [Any], let kind = a.first as? String else { return .none }
            if kind == "index", a.count > 1, let n = a[1] as? Int { return .index(n) }
            if kind == "rgb", a.count > 1, let r = a[1] as? [Any], r.count == 3,
               let r0 = r[0] as? Int, let g0 = r[1] as? Int, let b0 = r[2] as? Int {
                return .rgb(AnsiRGB(UInt8(clamping: r0), UInt8(clamping: g0), UInt8(clamping: b0)))
            }
            return .none
        }
        var s = AnsiStyle()
        s.fg = color(d["fg"])
        s.bg = color(d["bg"])
        s.bold = d["bold"] as? Bool ?? false
        s.dim = d["dim"] as? Bool ?? false
        s.italic = d["italic"] as? Bool ?? false
        s.underline = d["underline"] as? Bool ?? false
        s.inverse = d["inverse"] as? Bool ?? false
        s.strike = d["strike"] as? Bool ?? false
        return s
    }
}

struct AnsiTheme {
    var foreground = AnsiRGB(0xc0, 0xca, 0xf5)
    var background = AnsiRGB(0x1a, 0x1b, 0x26)
    var palette: [AnsiRGB] = AnsiTheme.xterm256()
    var fontName = "Menlo"
    var fontSize: CGFloat = 13
    var boldIsBright = false
    var displayP3 = false

    static func xterm256() -> [AnsiRGB] {
        var p: [AnsiRGB] = [
            AnsiRGB(0x00, 0x00, 0x00), AnsiRGB(0xcd, 0x00, 0x00), AnsiRGB(0x00, 0xcd, 0x00), AnsiRGB(0xcd, 0xcd, 0x00),
            AnsiRGB(0x00, 0x00, 0xee), AnsiRGB(0xcd, 0x00, 0xcd), AnsiRGB(0x00, 0xcd, 0xcd), AnsiRGB(0xe5, 0xe5, 0xe5),
            AnsiRGB(0x7f, 0x7f, 0x7f), AnsiRGB(0xff, 0x00, 0x00), AnsiRGB(0x00, 0xff, 0x00), AnsiRGB(0xff, 0xff, 0x00),
            AnsiRGB(0x5c, 0x5c, 0xff), AnsiRGB(0xff, 0x00, 0xff), AnsiRGB(0x00, 0xff, 0xff), AnsiRGB(0xff, 0xff, 0xff),
        ]
        let steps: [UInt8] = [0, 95, 135, 175, 215, 255]
        for r in steps { for g in steps { for b in steps { p.append(AnsiRGB(r, g, b)) } } }
        for i in 0..<24 { let v = UInt8(8 + i * 10); p.append(AnsiRGB(v, v, v)) }
        return p
    }

    static func ghostty(_ text: String) -> AnsiTheme {
        var t = AnsiTheme()
        var fontSet = false
        for line in text.split(separator: "\n") {
            guard let r = line.range(of: " = ") else { continue }
            let k = line[..<r.lowerBound].trimmingCharacters(in: .whitespaces)
            let v = line[r.upperBound...].trimmingCharacters(in: .whitespaces)
            switch k {
            case "background": if let c = AnsiRGB(hex: v) { t.background = c }
            case "foreground": if let c = AnsiRGB(hex: v) { t.foreground = c }
            case "font-family": if !fontSet, !v.isEmpty { t.fontName = v; fontSet = true }
            case "font-size": if let s = Double(v), s > 0 { t.fontSize = CGFloat(s) }
            case "bold-is-bright": t.boldIsBright = v == "true"
            case "window-colorspace": t.displayP3 = v == "display-p3"
            case "palette":
                let parts = v.split(separator: "=", maxSplits: 1)
                if parts.count == 2, let n = Int(parts[0]), (0..<256).contains(n), let c = AnsiRGB(hex: String(parts[1])) {
                    t.palette[n] = c
                }
            default: break
            }
        }
        return t
    }

    func colors(_ s: AnsiStyle) -> (fg: AnsiRGB, bg: AnsiRGB?) {
        func res(_ c: AnsiColor, bright: Bool) -> AnsiRGB? {
            switch c {
            case .none: return nil
            case .index(let n): return palette[bright && n < 8 ? n + 8 : n]
            case .rgb(let v): return v
            }
        }
        var fg = res(s.fg, bright: boldIsBright && s.bold) ?? foreground
        var bg = res(s.bg, bright: false)
        if s.inverse {
            let f = fg
            fg = bg ?? background
            bg = f
        }
        return (fg, bg)
    }
}

enum AnsiRender {
    struct Metrics: Equatable {
        var cellWidth: CGFloat
        var lineHeight: CGFloat
        var descent: CGFloat
    }

    static func font(_ t: AnsiTheme) -> CTFont {
        let f = CTFontCreateWithName(t.fontName as CFString, t.fontSize, nil)
        if CTFontGetSymbolicTraits(f).contains(.traitMonoSpace) || (CTFontCopyFamilyName(f) as String) == t.fontName { return f }
        return CTFontCreateWithName("Menlo" as CFString, t.fontSize, nil)
    }

    static func metrics(_ f: CTFont) -> Metrics {
        var g = CGGlyph(0)
        var ch: UniChar = 0x4D
        CTFontGetGlyphsForCharacters(f, &ch, &g, 1)
        var adv = CGSize.zero
        CTFontGetAdvancesForGlyphs(f, .horizontal, &g, &adv, 1)
        let lh = ceil(CTFontGetAscent(f) + CTFontGetDescent(f) + CTFontGetLeading(f))
        return Metrics(cellWidth: adv.width, lineHeight: lh, descent: ceil(CTFontGetDescent(f)))
    }

    static func size(_ grid: AnsiGrid, _ m: Metrics, padding: CGFloat) -> CGSize {
        CGSize(width: ceil(CGFloat(grid.columns) * m.cellWidth + padding * 2),
               height: CGFloat(grid.rows.count) * m.lineHeight + padding * 2)
    }

    static func scale(for pts: CGSize, wanted: CGFloat, maxPixels: CGFloat = 32000) -> CGFloat {
        max(pts.width, pts.height) * wanted > maxPixels ? 1 : wanted
    }

    static func image(_ grid: AnsiGrid, theme t: AnsiTheme, padding: CGFloat = 16, scale wanted: CGFloat = 2) -> CGImage? {
        let base = font(t)
        let m = metrics(base)
        let pts = size(grid, m, padding: padding)
        let sc = scale(for: pts, wanted: wanted)
        let space = CGColorSpace(name: t.displayP3 ? CGColorSpace.displayP3 : CGColorSpace.sRGB)!
        guard pts.width > 0, pts.height > 0,
              let ctx = CGContext(data: nil, width: Int(pts.width * sc), height: Int(pts.height * sc),
                                  bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.scaleBy(x: sc, y: sc)
        func cg(_ c: AnsiRGB, _ a: CGFloat = 1) -> CGColor {
            CGColor(colorSpace: space, components: [CGFloat(c.r) / 255, CGFloat(c.g) / 255, CGFloat(c.b) / 255, a])!
        }
        ctx.setFillColor(cg(t.background))
        ctx.fill(CGRect(origin: .zero, size: pts))
        ctx.setShouldSmoothFonts(false)
        ctx.textMatrix = .identity

        var fonts: [Int: CTFont] = [0: base]
        func variant(_ s: AnsiStyle) -> CTFont {
            let key = (s.bold ? 1 : 0) | (s.italic ? 2 : 0)
            if let f = fonts[key] { return f }
            var traits = CTFontSymbolicTraits()
            if s.bold { traits.insert(.traitBold) }
            if s.italic { traits.insert(.traitItalic) }
            let f = CTFontCreateCopyWithSymbolicTraits(base, 0, nil, traits, traits) ?? base
            fonts[key] = f
            return f
        }
        let ulPos = CTFontGetUnderlinePosition(base), ulThick = max(1, CTFontGetUnderlineThickness(base))

        for (r, row) in grid.rows.enumerated() {
            let top = pts.height - padding - CGFloat(r + 1) * m.lineHeight
            let baseline = top + m.descent
            for (c, cell) in row.enumerated() {
                guard let bg = t.colors(cell.style).bg else { continue }
                ctx.setFillColor(cg(bg))
                ctx.fill(CGRect(x: padding + CGFloat(c) * m.cellWidth, y: top, width: m.cellWidth, height: m.lineHeight).integral)
            }
            for (c, cell) in row.enumerated() where !cell.text.isEmpty {
                let x = padding + CGFloat(c) * m.cellWidth
                let fg = t.colors(cell.style).fg
                let color = cg(fg, cell.style.dim ? 0.5 : 1)
                let w = CGFloat(max(cell.width, 1)) * m.cellWidth
                if cell.style.underline {
                    ctx.setFillColor(color)
                    ctx.fill(CGRect(x: x, y: baseline + ulPos - ulThick / 2, width: w, height: ulThick))
                }
                if cell.style.strike {
                    ctx.setFillColor(color)
                    ctx.fill(CGRect(x: x, y: baseline + CTFontGetXHeight(base) / 2, width: w, height: ulThick))
                }
                if cell.text == " " { continue }
                let f = variant(cell.style)
                let utf16 = Array(cell.text.utf16)
                var glyphs = [CGGlyph](repeating: 0, count: utf16.count)
                if cell.text.unicodeScalars.count == 1, CTFontGetGlyphsForCharacters(f, utf16, &glyphs, utf16.count) {
                    ctx.setFillColor(color)
                    ctx.textPosition = .zero
                    var pos = CGPoint(x: x, y: baseline)
                    CTFontDrawGlyphs(f, glyphs, &pos, 1, ctx)
                } else {
                    let attr = NSAttributedString(string: cell.text, attributes: [
                        kCTFontAttributeName as NSAttributedString.Key: f,
                        kCTForegroundColorAttributeName as NSAttributedString.Key: color,
                    ])
                    let line = CTLineCreateWithAttributedString(attr)
                    let lw = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
                    ctx.textPosition = CGPoint(x: x + max(0, (w - lw) / 2), y: baseline)
                    CTLineDraw(line, ctx)
                }
            }
        }
        return ctx.makeImage()
    }
}
