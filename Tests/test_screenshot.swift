// sources: ScreenshotAnnotations.swift ScreenshotText.swift PythonHelper.swift
import Foundation
import CoreGraphics
import CoreText

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL (line \(line)): \(message)")
    }
}

func wsLog(_ s: String) {
    if ProcessInfo.processInfo.environment["WS_TEST_LOG"] != nil { print("log: \(s)") }
}

func near(_ a: CGFloat, _ b: CGFloat, _ eps: CGFloat = 0.01) -> Bool { abs(a - b) <= eps }

@main
struct ScreenshotTests {
    static func main() {
        let libDir = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("pylib").path
        PythonHelper.shared.configure(libDir: libDir)
        ringTests()
        documentTests()
        snapTests()
        pixelateTests()
        renderTests()
        colorTests()
        ocrTests()
        print("screenshot: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }

    static func noOverlap(_ fs: [CGRect]) -> Bool {
        for i in fs.indices { for j in fs.indices where j > i && fs[i].insetBy(dx: 0.5, dy: 0.5).intersects(fs[j]) { return false } }
        return true
    }

    static func ringTests() {
        let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
        // the ring composition + the file/args cases moved to the python
        // suite (ws test shot-model); this suite keeps the layout math
        let count = 21  // the default ring: 20 buttons + the size badge

        let B: CGFloat = 34
        let sel = CGRect(x: 500, y: 300, width: 400, height: 250)
        let l = ButtonRing.layout(selection: sel, screen: screen, count: count, button: B)
        check(l.frames.count == count && !l.inside, "centered: every button placed, outside")
        check(l.frames.allSatisfy { screen.contains($0) }, "centered: all on screen")
        check(l.frames.allSatisfy { !$0.intersects(sel) }, "centered: none covers the selection")
        check(noOverlap(l.frames), "centered: no two buttons overlap")
        let perRow = Int((sel.width + (B / 4).rounded(.down)) / (B + (B / 4).rounded(.down)))
        check(l.frames.prefix(perRow).allSatisfy { $0.minY >= sel.maxY }, "bottom row first")
        check(zip(l.frames.prefix(perRow), l.frames.prefix(perRow).dropFirst()).allSatisfy { $0.minX < $1.minX }, "bottom row runs left to right")
        let right = l.frames[perRow]
        check(right.minX >= sel.maxX, "then the right column")
        check(l.frames[perRow + 1].minY < right.minY, "right column runs bottom up")

        let edges: [(String, CGRect)] = [
            ("left", CGRect(x: 0, y: 300, width: 300, height: 200)),
            ("right", CGRect(x: 1140, y: 300, width: 300, height: 200)),
            ("top", CGRect(x: 500, y: 0, width: 300, height: 200)),
            ("bottom", CGRect(x: 500, y: 700, width: 300, height: 200)),
            ("top-left corner", CGRect(x: 0, y: 0, width: 200, height: 150)),
            ("bottom-right corner", CGRect(x: 1240, y: 750, width: 200, height: 150)),
        ]
        for (name, s) in edges {
            let e = ButtonRing.layout(selection: s, screen: screen, count: count, button: B)
            check(e.frames.count == count && e.frames.allSatisfy { $0.width == B }, "\(name): every button placed")
            check(e.frames.allSatisfy { screen.contains($0) }, "\(name): all on screen")
            check(noOverlap(e.frames), "\(name): no overlap")
        }

        let full = ButtonRing.layout(selection: screen, screen: screen, count: count, button: B)
        check(full.inside, "full-screen selection puts the buttons inside")
        check(full.frames.allSatisfy { screen.contains($0) && $0.width == B }, "inside: all on screen")
        check(noOverlap(full.frames), "inside: no overlap")
        check(full.frames.first.map { $0.maxY >= screen.maxY - B - 10 } ?? false, "inside: first row at the bottom edge")

        let tiny = CGRect(x: 700, y: 400, width: 4, height: 4)
        let t = ButtonRing.layout(selection: tiny, screen: screen, count: count, button: B)
        check(t.frames.allSatisfy { screen.contains($0) && $0.width == B } && noOverlap(t.frames), "tiny selection: placed, no overlap")
        let tinyCorner = CGRect(x: 0, y: 0, width: 6, height: 6)
        let tc = ButtonRing.layout(selection: tinyCorner, screen: screen, count: count, button: B)
        check(tc.frames.allSatisfy { screen.contains($0) && $0.width == B } && noOverlap(tc.frames), "tiny selection in a corner")
        check(ButtonRing.layout(selection: sel, screen: screen, count: 0, button: B).frames.isEmpty, "no buttons")
        check(ButtonRing.defaultButtonSize(lineHeight: 15.5) == 34, "button size = line height × 2.2")
    }

    static func counter(_ x: CGFloat) -> ShotObject {
        ShotObject(tool: .counter, points: [CGPoint(x: x, y: 10)], color: .black, size: 1)
    }

    static func documentTests() {
        let d = ShotDocument(undoLimit: 100)
        d.add(counter(10)); d.add(counter(100)); d.add(counter(200))
        check(d.objects.map(\.number) == [1, 2, 3], "counters 1, 2, 3")
        d.remove(at: 1)
        check(d.objects.map(\.number) == [1, 2], "delete renumbers the later ones")
        d.undo()
        check(d.objects.map(\.number) == [1, 2, 3], "undo brings it back, renumbered")
        d.redo()
        check(d.objects.map(\.number) == [1, 2], "redo deletes again")
        d.undo()
        d.add(ShotObject(tool: .line, points: [.zero, CGPoint(x: 5, y: 5)], color: .black, size: 3))
        d.undo()
        check(d.objects.count == 3, "undo of an add")
        check(d.nextCounterNumber() == 4 && d.nextCounterNumber(offset: 6) == 10, "next counter (+ wheel offset)")
        var jump = counter(300)
        jump.numberOffset = 6
        d.add(jump)
        check(d.objects.last?.number == 10, "a bubble placed after a wheel jump")
        d.remove(at: 0)
        check(d.objects.map(\.number) == [1, 2, 9], "renumber keeps the jump")
        check(ShotDocument(undoLimit: 5).nextCounterNumber() == 1, "the first bubble is 1")

        let lim = ShotDocument(undoLimit: 3)
        for i in 0..<5 { lim.add(counter(CGFloat(i * 50))) }
        check(lim.undoDepth == 3, "undo stack capped at undo-limit (\(lim.undoDepth))")
        while lim.undo() {}
        check(lim.objects.count == 2, "only undo-limit steps back (\(lim.objects.count))")
        check(lim.canRedo, "redo after undo")
        lim.add(counter(1))
        check(!lim.canRedo, "a new change clears redo")

        let m = ShotDocument()
        m.add(ShotObject(tool: .selection, points: [CGPoint(x: 10, y: 10), CGPoint(x: 50, y: 50)], color: .black, size: 3))
        m.update(at: 0) { $0 = $0.moved(by: CGVector(dx: 5, dy: 5)) }
        m.update(at: 0) { $0.color = .white }
        for _ in 0..<4 { m.update(at: 0, coalesce: "size0") { $0.size += 1 } }
        check(m.objects[0].size == 7, "size changed")
        m.undo()
        check(m.objects[0].size == 3, "wheel notches undo as one step")
        m.undo()
        check(m.objects[0].color == .black, "color change undone")
        m.undo()
        check(m.objects[0].start == CGPoint(x: 10, y: 10), "move undone")

        let h = ShotDocument()
        h.add(ShotObject(tool: .selection, points: [CGPoint(x: 100, y: 100), CGPoint(x: 200, y: 200)], color: .black, size: 3))
        h.add(ShotObject(tool: .line, points: [CGPoint(x: 0, y: 0), CGPoint(x: 50, y: 0)], color: .black, size: 3))
        check(h.hit(CGPoint(x: 100, y: 150)) == 0, "outline rect hit on its edge")
        check(h.hit(CGPoint(x: 150, y: 150)) == nil, "outline rect not hit in its middle")
        check(h.hit(CGPoint(x: 25, y: 2)) == 1, "line hit near it")
        h.reorder(from: 1, to: 0)
        check(h.objects[0].tool == .line, "reorder (Layers)")
    }

    static func snapTests() {
        let o = CGPoint.zero
        let a = ShotSnap.angle(from: o, to: CGPoint(x: 100, y: 7))
        check(near(a.y, 0) && near(a.x, hypot(100, 7)), "near-horizontal snaps to 0°")
        let b = ShotSnap.angle(from: o, to: CGPoint(x: 50, y: 47))
        check(near(b.x, b.y), "near-diagonal snaps to 45°")
        let c = ShotSnap.angle(from: o, to: CGPoint(x: -3, y: -80))
        check(near(c.x, 0) && c.y < 0, "near-vertical snaps to 90°")
        for k in 0..<8 {
            let ang = Double(k) * .pi / 4 + 0.1
            let s = ShotSnap.angle(from: o, to: CGPoint(x: cos(ang) * 50, y: sin(ang) * 50))
            let got = atan2(Double(s.y), Double(s.x))
            let want = Double(k) * .pi / 4
            check(abs(remainder(got - want, 2 * .pi)) < 1e-6, "8 directions: \(k * 45)°")
        }
        let sq = ShotSnap.square(from: CGPoint(x: 10, y: 10), to: CGPoint(x: 40, y: -5))
        check(sq == CGPoint(x: 40, y: -20), "square keeps the signs, longer side")
        check(ShotSnap.snap(.pencil, from: o, to: CGPoint(x: 3, y: 4)) == CGPoint(x: 3, y: 4), "pencil never snaps")
    }

    static func pixels(w: Int, h: Int, interior: CGRect, _ inner: (Int, Int) -> [UInt8], _ outer: (Int, Int) -> [UInt8]) -> ShotPixels {
        var data = [UInt8](repeating: 255, count: w * h * 4)
        for y in 0..<h {
            for x in 0..<w {
                let c = interior.contains(CGPoint(x: Double(x) + 0.5, y: Double(y) + 0.5)) ? inner(x, y) : outer(x, y)
                let i = (y * w + x) * 4
                data[i] = c[0]; data[i + 1] = c[1]; data[i + 2] = c[2]; data[i + 3] = 255
            }
        }
        return ShotPixels(width: w, height: h, data: data)
    }

    static func pixelateTests() {
        let interior = CGRect(x: 30, y: 30, width: 40, height: 40)
        let px = pixels(w: 100, h: 100, interior: interior, { _, _ in [0, 255, 0] },
                        { x, y in [UInt8(120 + x), UInt8(60 + y / 2), UInt8(60)] })
        for size in [1, 2, 5, 20] {
            let blocks = ShotPixelate.secureBlocks(px, px: interior, size: size)
            let flat = blocks.flatMap { $0 }
            let (cols, rows) = ShotPixelate.grid(interior, size: size)
            check(blocks.count == rows && blocks.allSatisfy { $0.count == cols }, "size \(size): grid \(cols)×\(rows)")
            let leaked = flat.contains { $0.g > 0.6 && $0.r < 0.3 }
            check(!leaked, "size \(size): no block carries the hidden interior color")
        }
        let edge = pixels(w: 60, h: 60, interior: CGRect(x: 0, y: 0, width: 30, height: 30), { _, _ in [0, 255, 0] }, { _, _ in [200, 40, 40] })
        let eb = ShotPixelate.secureBlocks(edge, px: CGRect(x: 0, y: 0, width: 30, height: 30), size: 2).flatMap { $0 }
        check(!eb.contains { $0.g > 0.6 }, "rect at the image corner: no leak")
        let retina = ShotPixelate.secureBlocks(px, px: interior, size: 2, scale: 2)
        let pts = ShotPixelate.grid(CGRect(x: 0, y: 0, width: 20, height: 20), size: 2)
        check(retina.count == pts.rows && retina.first?.count == pts.cols, "2x display: the block grid is in points")
        let grid = ShotPixelate.grid(CGRect(x: 0, y: 0, width: 300, height: 120), size: 2)
        check(grid.cols == 50 && grid.rows == 20, "block resolution = rect × 0.5 / (size + 1)")
    }

    static func image(w: Int, h: Int, _ f: (Int, Int) -> [UInt8]) -> CGImage {
        var data = [UInt8](repeating: 0, count: w * h * 4)
        for y in 0..<h { for x in 0..<w {
            let c = f(x, y), i = (y * w + x) * 4
            data[i] = c[0]; data[i + 1] = c[1]; data[i + 2] = c[2]; data[i + 3] = 255
        } }
        let ctx = CGContext(data: &data, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        return ctx.makeImage()!
    }

    static func renderTests() {
        let img = image(w: 400, h: 200) { x, _ in x < 200 ? [255, 0, 0] : [0, 0, 255] }
        let canvas = ShotCanvas(base: img, scale: 2)
        check(canvas.size == CGSize(width: 200, height: 100), "canvas size in points")
        let crop = CGRect(x: 10, y: 20, width: 40, height: 30)
        let out = ShotRenderer.render(canvas, crop: crop, objects: [])
        check(out?.width == 80 && out?.height == 60, "output = rect × backing scale (\(out?.width ?? 0)×\(out?.height ?? 0))")
        if let out, let p = ShotPixels(out) {
            let c = p.color(5, 5)
            check(c.r > 0.9 && c.b < 0.1, "crop of the left half is red")
        }
        let tb = image(w: 100, h: 100) { _, y in y < 50 ? [0, 255, 0] : [0, 0, 0] }
        let tbc = ShotCanvas(base: tb, scale: 1)
        if let o = ShotRenderer.render(tbc, crop: CGRect(x: 0, y: 0, width: 100, height: 10), objects: []),
           let p = ShotPixels(o) {
            check(p.color(50, 5).g > 0.9, "crop y is measured from the top")
        }
        let odd = ShotRenderer.render(ShotCanvas(base: img, scale: 2), crop: CGRect(x: 0.5, y: 0, width: 33.5, height: 17), objects: [])
        check(odd?.width == 67 && odd?.height == 34, "fractional rect × scale")
        check(ShotRenderer.render(canvas, crop: CGRect(x: 500, y: 500, width: 10, height: 10), objects: []) == nil, "off-canvas crop = nil")

        let all: [ShotObject] = ShotTool.allCases.filter(\.isDrawing).map { t in
            var o = ShotObject(tool: t, points: [CGPoint(x: 20, y: 20), CGPoint(x: 120, y: 80)], color: ShotColor(r: 0, g: 1, b: 0), size: 4)
            if t == .text { o.text = "Hello\nworld"; o.points = [CGPoint(x: 20, y: 20)] }
            if t == .pencil { o.points = [CGPoint(x: 1, y: 1), CGPoint(x: 5, y: 9), CGPoint(x: 30, y: 3)] }
            return o
        }
        let full = ShotRenderer.render(canvas, crop: CGRect(x: 0, y: 0, width: 200, height: 100), objects: all)
        check(full?.width == 400, "every tool renders")
        let rect = ShotObject(tool: .rectangle, points: [CGPoint(x: 10, y: 10), CGPoint(x: 30, y: 30)], color: ShotColor(r: 0, g: 1, b: 0), size: 0)
        if let o = ShotRenderer.render(canvas, crop: CGRect(x: 0, y: 0, width: 50, height: 50), objects: [rect]), let p = ShotPixels(o) {
            check(p.color(40, 40).g > 0.9 && p.color(70, 70).r > 0.9, "rectangle drawn at 2× in the right place")
        }
        let secret = image(w: 100, h: 100) { x, y in (30..<70).contains(x) && (30..<70).contains(y) ? [0, 255, 0] : [180, 30, 30] }
        let sc = ShotCanvas(base: secret, scale: 1)
        let pix = ShotObject(tool: .pixelate, points: [CGPoint(x: 30, y: 30), CGPoint(x: 70, y: 70)], color: .black, size: 2)
        if let o = ShotRenderer.render(sc, crop: CGRect(x: 0, y: 0, width: 100, height: 100), objects: [pix]), let p = ShotPixels(o) {
            var leak = false
            for y in 30..<70 { for x in 30..<70 where p.color(x, y).g > 0.6 { leak = true } }
            check(!leak, "rendered secure pixelate shows no hidden pixel")
        }
        var t = ShotObject(tool: .text, points: [.zero], color: .black, size: 8, text: "a")
        let small = ShotText.boxSize(t)
        t.text = "a much longer line\nand a second"
        let big = ShotText.boxSize(t)
        check(big.width > small.width && big.height > small.height, "text box grows")
    }

    static func colorTests() {
        check(ShotColor(hex: "#740096")?.hex == "#740096", "hex round trip")
        check(ShotColor(hex: "#740096")?.isDark == true, "Flameshot purple is dark → white icons")
        check(ShotColor(hex: "#ffff00")?.isDark == false, "yellow is light → black icons")
        check(ShotColor(hex: "zz") == nil && ShotColor(hex: "#12345") == nil, "bad hex rejected")
        let c = ShotColor(hex: "#3366cc")!
        let hsv = c.hsv
        check(ShotColor(h: hsv.h, s: hsv.s, v: hsv.v).hex == "#3366cc", "HSV round trip")
    }

    static func line(_ t: String, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat = 100, _ h: CGFloat = 20) -> ShotOCRLine {
        ShotOCRLine(text: t, box: CGRect(x: x, y: y, width: w, height: h))
    }

    static func textImage(_ lines: [(String, CGFloat, CGFloat)], size: CGSize, font: CGFloat = 15) -> CGImage {
        let k: CGFloat = 2
        let ctx = CGContext(data: nil, width: Int(size.width * k), height: Int(size.height * k), bitsPerComponent: 8,
                            bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: size.width * k, height: size.height * k))
        let f = CTFontCreateWithName("Helvetica" as CFString, font * k, nil)
        for (t, x, y) in lines {
            let attr = NSAttributedString(string: t, attributes: [kCTFontAttributeName as NSAttributedString.Key: f,
                                                                   kCTForegroundColorAttributeName as NSAttributedString.Key: CGColor(red: 0, green: 0, blue: 0, alpha: 1)])
            let l = CTLineCreateWithAttributedString(attr)
            ctx.textPosition = CGPoint(x: x * k, y: (size.height - y - font) * k)
            CTLineDraw(l, ctx)
        }
        return ctx.makeImage()!
    }

    static func ocrTests() {
        check(ShotOCR.join([]) == "", "nothing → empty")
        check(ShotOCR.join([line("second", 0, 30), line("first", 0, 0)]) == "first\nsecond", "rows top → bottom")
        check(ShotOCR.join([line("right", 200, 2), line("left", 0, 0)]) == "left right", "one row, left → right")
        check(ShotOCR.join([line("a", 0, 0), line("b", 0, 25), line("c", 0, 90)]) == "a\nb\n\nc", "a tall gap = a paragraph")
        check(ShotOCR.join([line("  pad  ", 0, 0)]) == "pad", "trimmed")
        check(ShotOCR.summary("one line") == "8 characters" && ShotOCR.summary("a\nb\nc") == "3 lines", "toast summary")

        let img = textImage([("Copy Text reads the screen", 20, 20), ("Second line here", 20, 48),
                             ("A new paragraph", 20, 120)], size: CGSize(width: 420, height: 170))
        let got = ShotOCR.text(img)
        check(got == "Copy Text reads the screen\nSecond line here\n\nA new paragraph", "OCR + layout, got: \(got.debugDescription)")
        let small = textImage([("Invoice 4815162342", 4, 4)], size: CGSize(width: 200, height: 24), font: 12)
        let s2 = ShotOCR.text(small)
        check(s2 == "Invoice 4815162342", "small crop, got: \(s2.debugDescription)")
        check(ShotOCR.text(textImage([], size: CGSize(width: 100, height: 100))) == "", "blank → no text")
    }
}
