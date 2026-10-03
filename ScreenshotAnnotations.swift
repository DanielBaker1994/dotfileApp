import Foundation
import CoreGraphics
import CoreText
import CoreImage

// /screenshot's model — no AppKit (bin/run-tests.sh screenshot builds this
// file alone): the tools, the drawn objects + their undo stack, the button
// ring's placement (a port of Flameshot's ButtonHandler), Shift snapping,
// the pixelate algorithms, the renderer shared by the overlay and the
// output image, the filename pattern and the CLI arguments.
//
// Coordinates: points of ONE display, TOP-LEFT origin (the overlay view is
// flipped, Flameshot's convention, `--region` too). Every draw function
// expects a context whose user space is that (see `ShotRenderer.render`).

// MARK: - Tools

enum ShotTool: String, CaseIterable {
    // drawing tools (the bottom row, Flameshot's buttonTypeOrder)
    case pencil, line, arrow, selection, rectangle, circle, marker, text, counter, pixelate, invert
    // the rest of the ring
    case badge = "size"            // W / H of the selection (Flameshot's selection indicator)
    case move, undo, redo, copy, save, accept, exit, pin
    case copyText = "copy-text"   // OCR the selection → the clipboard (Copy Text mode)
    case sizeUp = "size-increase", sizeDown = "size-decrease"

    enum Kind { case draw, mode, action, info }

    var kind: Kind {
        switch self {
        case .pencil, .line, .arrow, .selection, .rectangle, .circle, .marker, .text, .counter, .pixelate, .invert:
            return .draw
        case .move: return .mode
        case .badge: return .info
        default: return .action
        }
    }
    var isDrawing: Bool { kind == .draw }
    // closes the capture when used
    var finishes: Bool { [.copy, .save, .accept, .exit, .pin, .copyText].contains(self) }

    // the single-letter key (capture mode, no modifier)
    var letter: Character? {
        switch self {
        case .pencil: return "p"
        case .line: return "d"
        case .arrow: return "a"
        case .selection: return "s"
        case .rectangle: return "r"
        case .circle: return "c"
        case .marker: return "m"
        case .text: return "t"
        case .pixelate: return "b"
        case .invert: return "i"
        default: return nil
        }
    }
    static func forLetter(_ c: Character) -> ShotTool? {
        allCases.first { $0.letter == Character(c.lowercased()) }
    }

    var symbol: String {
        switch self {
        case .pencil: return "pencil"
        case .line: return "line.diagonal"
        case .arrow: return "arrow.down.left"
        case .selection: return "square"
        case .rectangle: return "square.fill"
        case .circle: return "circle"
        case .marker: return "highlighter"
        case .text: return "textformat"
        case .counter: return "1.circle"
        case .pixelate: return "square.grid.3x3.fill"
        case .invert: return "circle.lefthalf.filled"
        case .badge: return ""
        case .move: return "arrow.up.and.down.and.arrow.left.and.right"
        case .undo: return "arrow.uturn.backward"
        case .redo: return "arrow.uturn.forward"
        case .copy: return "doc.on.doc"
        case .save: return "square.and.arrow.down"
        case .accept: return "checkmark"
        case .exit: return "xmark"
        case .pin: return "pin.fill"
        case .copyText: return "text.viewfinder"
        case .sizeUp: return "plus"
        case .sizeDown: return "minus"
        }
    }

    // Flameshot's descriptions (the button tooltips)
    var tooltip: String {
        switch self {
        case .pencil: return "Set the Pencil as the paint tool (P)"
        case .line: return "Set the Line as the paint tool (D)"
        case .arrow: return "Set the Arrow as the paint tool (A)"
        case .selection: return "Set Selection as the paint tool (S)"
        case .rectangle: return "Set the Rectangle as the paint tool (R)"
        case .circle: return "Set the Circle as the paint tool (C)"
        case .marker: return "Set the Marker as the paint tool (M)"
        case .text: return "Add text to your capture (T)"
        case .counter: return "Add an autoincrementing counter bubble"
        case .pixelate: return "Set Pixelate as the paint tool (B)"
        case .invert: return "Set Inverter as the paint tool (I)"
        case .badge: return "Selection size"
        case .move: return "Move the selection area (⌘M)"
        case .undo: return "Undo the last modification (⌘Z)"
        case .redo: return "Redo the next modification (⇧⌘Z)"
        case .copy: return "Copy selection to clipboard (⌘C)"
        case .save: return "Save screenshot to a file (⌘S)"
        case .accept: return "Accept the capture (Return)"
        case .exit: return "Leave the capture screen (⌘Q)"
        case .pin: return "Pin image on the desktop"
        case .copyText: return "Copy the text in the selection (⇧⌘C)"
        case .sizeUp: return "Increase tool size"
        case .sizeDown: return "Decrease tool size"
        }
    }

    // Flameshot's per-tool size defaults (drawThickness 3, drawFontSize 8,
    // drawMarkerSize 5, drawPixelateSize 2, drawCircleCounterSize 1,
    // drawRectangleSize 1 = the filled rectangle's corner radius)
    var defaultSize: Int {
        switch self {
        case .text: return 8
        case .marker: return 5
        case .pixelate: return 2
        case .counter: return 1
        case .rectangle: return 1
        default: return 3
        }
    }
    var sizeRange: ClosedRange<Int> { self == .rectangle ? 0...100 : 1...100 }

    // the ring's default order on macOS (12.1 observed): uploader / open-app
    // left out, accept + size buttons hidden
    static let defaultButtons = "pencil, line, arrow, selection, rectangle, circle, marker, text, counter, pixelate, invert, move, undo, redo, copy, copy-text, save, exit, pin"

    // `[screenshot] buttons` → the ring, in order; the size badge goes
    // right after the last drawing tool unless listed ("size") or hidden
    static func ring(_ spec: String, badge: Bool) -> [ShotTool] {
        var out: [ShotTool] = []
        for part in spec.split(separator: ",") {
            let n = part.trimmingCharacters(in: .whitespaces).lowercased()
            if let t = ShotTool(rawValue: n), !out.contains(t) { out.append(t) }
        }
        if out.isEmpty { return ring(defaultButtons, badge: badge) }
        if !badge { out.removeAll { $0 == .badge } }
        else if !out.contains(.badge) {
            let at = (out.lastIndex { $0.isDrawing }).map { $0 + 1 } ?? 0
            out.insert(.badge, at: at)
        }
        return out
    }
}

// MARK: - Color

struct ShotColor: Equatable, Codable {
    var r: Double, g: Double, b: Double, a: Double = 1

    init(r: Double, g: Double, b: Double, a: Double = 1) {
        self.r = r; self.g = g; self.b = b; self.a = a
    }
    // #RRGGBB / RRGGBB / #AARRGGBB
    init?(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6 || s.count == 8, let v = UInt32(s, radix: 16) else { return nil }
        let a = s.count == 8 ? Double((v >> 24) & 0xff) / 255 : 1
        self.init(r: Double((v >> 16) & 0xff) / 255, g: Double((v >> 8) & 0xff) / 255,
                  b: Double(v & 0xff) / 255, a: a)
    }
    var hex: String {
        func c(_ x: Double) -> Int { Int((max(0, min(1, x)) * 255).rounded()) }
        return String(format: "#%02x%02x%02x", c(r), c(g), c(b))
    }
    var cgColor: CGColor {
        CGColor(srgbRed: CGFloat(r), green: CGFloat(g), blue: CGFloat(b), alpha: CGFloat(a))
    }
    func with(alpha: Double) -> ShotColor { ShotColor(r: r, g: g, b: b, a: alpha) }
    // Flameshot's colorIsDark: perceived luminance below one half
    var luminance: Double { 0.299 * r + 0.587 * g + 0.114 * b }
    var isDark: Bool { luminance < 0.5 }
    // a lighter / darker shade (hover)
    func mixed(with o: ShotColor, _ t: Double) -> ShotColor {
        ShotColor(r: r + (o.r - r) * t, g: g + (o.g - g) * t, b: b + (o.b - b) * t, a: a)
    }
    static let white = ShotColor(r: 1, g: 1, b: 1)
    static let black = ShotColor(r: 0, g: 0, b: 0)
    // HSV (the side panel's wheel)
    init(h: Double, s: Double, v: Double) {
        let i = Int(floor(h * 6)) % 6, f = h * 6 - floor(h * 6)
        let p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s)
        switch i {
        case 0: self.init(r: v, g: t, b: p)
        case 1: self.init(r: q, g: v, b: p)
        case 2: self.init(r: p, g: v, b: t)
        case 3: self.init(r: p, g: q, b: v)
        case 4: self.init(r: t, g: p, b: v)
        default: self.init(r: v, g: p, b: q)
        }
    }
    var hsv: (h: Double, s: Double, v: Double) {
        let mx = max(r, g, b), mn = min(r, g, b), d = mx - mn
        var h = 0.0
        if d > 0 {
            if mx == r { h = ((g - b) / d).truncatingRemainder(dividingBy: 6) }
            else if mx == g { h = (b - r) / d + 2 }
            else { h = (r - g) / d + 4 }
            h /= 6
            if h < 0 { h += 1 }
        }
        return (h, mx == 0 ? 0 : d / mx, mx)
    }
}

// MARK: - Objects

struct ShotTextStyle: Equatable, Codable {
    var family = ""            // "" = the system font
    var bold = false, italic = false, underline = false, strike = false
    var align = 0              // 0 left, 1 center, 2 right
}

struct ShotObject: Equatable {
    var tool: ShotTool
    // pencil: the polyline; two-point tools: [start, end];
    // text: [top-left]; counter: [center] or [center, tail tip]
    var points: [CGPoint]
    var color: ShotColor
    var size: Int
    var text = ""
    var style = ShotTextStyle()
    var numberOffset = 0       // counter: jump over the previous bubble + 1
    var number = 1             // counter: derived (ShotDocument.renumber)
    var openArrow = false      // arrow-style 1
    var reversed = false       // reverse-arrow
    var outline = true         // counter-outline
    var secure = true          // pixelate: not insecure-pixelate

    var start: CGPoint { points.first ?? .zero }
    var end: CGPoint { points.last ?? .zero }
    var rect: CGRect { ShotGeom.rect(start, end) }

    // stroke width the tool draws with
    var strokeWidth: CGFloat {
        switch tool {
        case .marker: return CGFloat(size * 2 + 2)
        default: return CGFloat(max(1, size))
        }
    }
    var fontSize: CGFloat { CGFloat(size + 8) }
    var counterRadius: CGFloat { CGFloat(size) + 12 }

    func moved(by d: CGVector) -> ShotObject {
        var o = self
        o.points = points.map { CGPoint(x: $0.x + d.dx, y: $0.y + d.dy) }
        return o
    }

    // what to draw around a selected object + what clicks hit
    var bbox: CGRect {
        switch tool {
        case .text:
            return CGRect(origin: start, size: ShotText.boxSize(self))
        case .counter:
            let r = counterRadius
            var b = CGRect(x: start.x - r, y: start.y - r, width: 2 * r, height: 2 * r)
            if points.count > 1 { b = b.union(CGRect(origin: end, size: .zero)) }
            return b
        case .pencil, .line, .arrow, .marker:
            var b = ShotGeom.bounds(points)
            let pad = strokeWidth / 2 + (tool == .arrow ? ShotArrow.headLength(size) / 2 : 0)
            b = b.insetBy(dx: -pad, dy: -pad)
            return b
        case .selection, .circle:
            return rect.insetBy(dx: -strokeWidth / 2, dy: -strokeWidth / 2)
        default:
            return rect
        }
    }

    // a click at `p` hits the drawn pixels (± `slop`), Flameshot-style
    func hit(_ p: CGPoint, slop: CGFloat = 4) -> Bool {
        let w = strokeWidth / 2 + slop
        switch tool {
        case .pencil, .marker:
            if points.count == 1 { return ShotGeom.dist(p, start) <= w }
            for i in 1..<points.count where ShotGeom.segDist(p, points[i - 1], points[i]) <= w { return true }
            return false
        case .line, .arrow:
            return ShotGeom.segDist(p, start, end) <= w
                || (tool == .arrow && ShotGeom.dist(p, reversed ? start : end) <= ShotArrow.headLength(size))
        case .selection:
            let r = rect
            return r.insetBy(dx: -w, dy: -w).contains(p) && !r.insetBy(dx: w, dy: w).contains(p)
        case .circle:
            let r = rect
            guard r.width > 0, r.height > 0 else { return ShotGeom.dist(p, r.origin) <= w }
            // normalized distance from the ellipse outline
            let dx = (p.x - r.midX) / (r.width / 2), dy = (p.y - r.midY) / (r.height / 2)
            let d = sqrt(dx * dx + dy * dy)
            let tol = w / max(1, min(r.width, r.height) / 2)
            return abs(d - 1) <= tol
        case .counter:
            if ShotGeom.dist(p, start) <= counterRadius + slop { return true }
            return points.count > 1 && ShotGeom.segDist(p, start, end) <= counterRadius / 2 + slop
        default:
            return bbox.insetBy(dx: -slop, dy: -slop).contains(p)
        }
    }
}

// MARK: - Document (objects + undo)

// The drawn objects of one capture. Every change goes through `commit`,
// which snapshots the list first: undo / redo walk whole snapshots
// (moves, deletes, color and size changes alike). Counters are renumbered
// after every change.
final class ShotDocument {
    private(set) var objects: [ShotObject] = []
    var selected: Int?
    var undoLimit: Int
    private var undoStack: [([ShotObject], String?)] = []
    private var redoStack: [[ShotObject]] = []
    private var lastCoalesce: String?

    init(undoLimit: Int = 100) { self.undoLimit = max(1, undoLimit) }

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }
    var undoDepth: Int { undoStack.count }

    // `coalesce`: consecutive commits with the same key (wheel notches on
    // one object) are one undo step
    func commit(coalesce: String? = nil, _ change: (inout [ShotObject]) -> Void) {
        if coalesce == nil || coalesce != lastCoalesce {
            undoStack.append((objects, coalesce))
            if undoStack.count > undoLimit { undoStack.removeFirst(undoStack.count - undoLimit) }
        }
        lastCoalesce = coalesce
        change(&objects)
        Self.renumber(&objects)
        redoStack.removeAll()
        if let s = selected, s >= objects.count { selected = nil }
    }

    @discardableResult
    func add(_ o: ShotObject) -> Int {
        commit { $0.append(o) }
        return objects.count - 1
    }
    func remove(at i: Int) {
        guard objects.indices.contains(i) else { return }
        commit { $0.remove(at: i) }
        selected = nil
    }
    func update(at i: Int, coalesce: String? = nil, _ f: (inout ShotObject) -> Void) {
        guard objects.indices.contains(i) else { return }
        commit(coalesce: coalesce) { f(&$0[i]) }
    }
    // Layers ↑/↓: `to` is the new index (0 = drawn first = bottom)
    func reorder(from: Int, to: Int) {
        guard objects.indices.contains(from), objects.indices.contains(to), from != to else { return }
        commit { let o = $0.remove(at: from); $0.insert(o, at: to) }
        selected = to
    }

    @discardableResult
    func undo() -> Bool {
        guard let (prev, _) = undoStack.popLast() else { return false }
        redoStack.append(objects)
        objects = prev
        lastCoalesce = nil
        selected = nil
        return true
    }
    @discardableResult
    func redo() -> Bool {
        guard let next = redoStack.popLast() else { return false }
        undoStack.append((objects, nil))
        objects = next
        lastCoalesce = nil
        selected = nil
        return true
    }

    // a change shown while dragging, without an undo step (mouse-up commits)
    func updateLive(at i: Int, _ f: (inout ShotObject) -> Void) {
        guard objects.indices.contains(i) else { return }
        f(&objects[i])
    }
    // a fresh start (the selection moved to another display)
    func reset() {
        objects = []
        undoStack = []
        redoStack = []
        selected = nil
        lastCoalesce = nil
    }

    // the topmost object under `p`
    func hit(_ p: CGPoint) -> Int? {
        objects.indices.reversed().first { objects[$0].hit(p) }
    }

    // the next bubble's number with `offset` (the wheel while placing)
    func nextCounterNumber(offset: Int = 0) -> Int {
        let last = objects.last { $0.tool == .counter }?.number ?? 0
        return min(999, max(1, last + 1 + offset))
    }

    // 1, 2, 3 … in drawing order (+ each bubble's own jump): deleting or
    // undoing a bubble renumbers the later ones
    static func renumber(_ objs: inout [ShotObject]) {
        var n = 0
        for i in objs.indices where objs[i].tool == .counter {
            n = min(999, max(1, n + 1 + objs[i].numberOffset))
            objs[i].number = n
        }
    }
}

// MARK: - Geometry + snapping

enum ShotGeom {
    static func rect(_ a: CGPoint, _ b: CGPoint) -> CGRect {
        CGRect(x: min(a.x, b.x), y: min(a.y, b.y), width: abs(b.x - a.x), height: abs(b.y - a.y))
    }
    static func bounds(_ pts: [CGPoint]) -> CGRect {
        guard let f = pts.first else { return .zero }
        var r = CGRect(origin: f, size: .zero)
        for p in pts.dropFirst() { r = r.union(CGRect(origin: p, size: .zero)) }
        return r
    }
    static func dist(_ a: CGPoint, _ b: CGPoint) -> CGFloat { hypot(a.x - b.x, a.y - b.y) }
    static func segDist(_ p: CGPoint, _ a: CGPoint, _ b: CGPoint) -> CGFloat {
        let dx = b.x - a.x, dy = b.y - a.y
        let l2 = dx * dx + dy * dy
        guard l2 > 0 else { return dist(p, a) }
        let t = max(0, min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l2))
        return dist(p, CGPoint(x: a.x + t * dx, y: a.y + t * dy))
    }
}

enum ShotSnap {
    // Shift on Line / Arrow: the end point snapped to the nearest of the 8
    // directions (0/45/90°…), keeping the length
    static func angle(from a: CGPoint, to b: CGPoint) -> CGPoint {
        let dx = b.x - a.x, dy = b.y - a.y
        let len = hypot(dx, dy)
        guard len > 0 else { return b }
        let step = CGFloat.pi / 4
        let ang = (atan2(dy, dx) / step).rounded() * step
        var x = a.x + cos(ang) * len, y = a.y + sin(ang) * len
        // exact on the axes (no 1e-14 drift)
        if abs(x - a.x) < 1e-9 { x = a.x }
        if abs(y - a.y) < 1e-9 { y = a.y }
        return CGPoint(x: x, y: y)
    }
    // Shift on rectangles / circle / pixelate: a square (diagonal snap)
    static func square(from a: CGPoint, to b: CGPoint) -> CGPoint {
        let dx = b.x - a.x, dy = b.y - a.y
        let s = max(abs(dx), abs(dy))
        return CGPoint(x: a.x + (dx < 0 ? -s : s), y: a.y + (dy < 0 ? -s : s))
    }
    static func snap(_ tool: ShotTool, from a: CGPoint, to b: CGPoint) -> CGPoint {
        switch tool {
        case .line, .arrow, .marker: return angle(from: a, to: b)
        case .selection, .rectangle, .circle, .pixelate, .invert: return square(from: a, to: b)
        default: return b
        }
    }
}

// MARK: - Button ring (Flameshot's ButtonHandler)

// Places `count` round buttons of size `button` around `selection` inside
// `screen` (both in points, top-left origin): bottom row first (left to
// right), right column (bottom up), top row (right to left), left column
// (top down); a side whose buttons would leave the screen is blocked;
// leftovers grow the base area one pitch and go around again; all sides
// blocked → rows INSIDE the selection along its bottom edge, then up.
// Returns each button's frame (top-left) in ring order.
enum ButtonRing {
    struct Layout {
        var frames: [CGRect]
        var inside: Bool
    }

    static func defaultButtonSize(lineHeight: CGFloat) -> CGFloat { (lineHeight * 2.2).rounded() }

    static func layout(selection: CGRect, screen: CGRect, count: Int, button: CGFloat) -> Layout {
        var frames = [CGRect?](repeating: nil, count: count)
        guard count > 0 else { return Layout(frames: [], inside: false) }
        let base = button
        let sep = (base / 4).rounded(.down)
        let ext = base + sep
        var sel = selection.intersection(screen)
        if sel.isNull { sel = CGRect(x: selection.minX, y: selection.minY, width: 0, height: 0) }
        var idx = 0
        var inside = false

        func place(_ pts: [CGPoint]) {
            for p in pts where idx < count {
                frames[idx] = CGRect(x: p.x, y: p.y, width: base, height: base)
                idx += 1
            }
        }
        func horizontal(_ c: CGPoint, _ n: Int, _ leftToRight: Bool) -> [CGPoint] {
            var shift: CGFloat = n % 2 == 0 ? ext * CGFloat(n / 2) - sep / 2
                                            : ext * CGFloat((n - 1) / 2) + base / 2
            if !leftToRight { shift -= base }
            var x = leftToRight ? c.x - shift : c.x + shift
            var out: [CGPoint] = []
            while out.count < n {
                out.append(CGPoint(x: x, y: c.y))
                x += leftToRight ? ext : -ext
            }
            return out
        }
        func vertical(_ c: CGPoint, _ n: Int, _ upToDown: Bool) -> [CGPoint] {
            var shift: CGFloat = n % 2 == 0 ? ext * CGFloat(n / 2) - sep / 2
                                            : ext * CGFloat((n - 1) / 2) + base / 2
            if !upToDown { shift -= base }
            var y = upToDown ? c.y - shift : c.y + shift
            var out: [CGPoint] = []
            while out.count < n {
                out.append(CGPoint(x: c.x, y: y))
                y += upToDown ? ext : -ext
            }
            return out
        }
        // (Flameshot's ensureSelectionMinimumSize)
        if sel.width < base { sel.origin.x -= ((base - sel.width) / 2).rounded(.down); sel.size.width = base }
        if sel.height < base { sel.origin.y -= ((base - sel.height) / 2).rounded(.down); sel.size.height = base }
        // (a grown tiny selection in a corner must not hang off the screen:
        // every side would read as blocked and nothing would fit inside)
        sel.origin.x = max(screen.minX, min(sel.minX, screen.maxX - sel.width))
        sel.origin.y = max(screen.minY, min(sel.minY, screen.maxY - sel.height))

        var guardLoops = 0
        while idx < count && guardLoops < 64 {
            guardLoops += 1
            // blocked sides (updateBlockedSides)
            let e = sep * 2 + base
            func onScreen(_ a: CGPoint, _ b: CGPoint) -> Bool {
                let s = screen.insetBy(dx: -0.5, dy: -0.5)
                return s.contains(a) && s.contains(b)
            }
            let bRight = !onScreen(CGPoint(x: sel.maxX + e, y: sel.maxY), CGPoint(x: sel.maxX + e, y: sel.minY))
            let bLeft = !onScreen(CGPoint(x: sel.minX - e, y: sel.maxY), CGPoint(x: sel.minX - e, y: sel.minY))
            let bBottom = !onScreen(CGPoint(x: sel.minX, y: sel.maxY + e), CGPoint(x: sel.maxX, y: sel.maxY + e))
            let bTop = !onScreen(CGPoint(x: sel.minX, y: sel.minY - e), CGPoint(x: sel.maxX, y: sel.minY - e))
            let oneHorizontal = bRight != bLeft
            let bothHorizontal = bRight && bLeft
            if bLeft && bothHorizontal && bBottom && bTop {
                // positionButtonsInside
                var area = sel.intersection(screen)
                if Int(area.width / ext) == 0 { area = screen }
                let perRow = Int(area.width / ext)
                guard perRow > 0 else { break }
                var c = CGPoint(x: area.midX, y: area.maxY - ext)
                while idx < count {
                    let n = min(perRow, count - idx)
                    place(horizontal(c, n, true))
                    c.y -= ext
                }
                inside = true
                break
            }
            let perRow = Int((sel.width + sep) / ext)
            let perCol = Int((sel.height + sep) / ext)
            let extra = (count - idx) - (perRow + perCol) * 2
            var corners = min(4, extra)
            let maxExtra = oneHorizontal ? 1 : bothHorizontal ? 0 : 2
            let cornersTop = max(0, min(corners, maxExtra))
            corners -= cornersTop
            let cornersBottom = max(0, min(corners, maxExtra))

            func adjust(_ c: inout CGPoint) {
                if bLeft { c.x += (ext / 2).rounded(.down) } else if bRight { c.x -= (ext / 2).rounded(.down) }
            }
            if !bBottom {
                let n = max(0, min(perRow + cornersBottom, count - idx))
                var c = CGPoint(x: sel.midX, y: sel.maxY + sep)
                if n > perRow { adjust(&c) }
                place(horizontal(c, n, true))
            }
            if !bRight && idx < count {
                let n = max(0, min(perCol, count - idx))
                place(vertical(CGPoint(x: sel.maxX + sep, y: sel.midY), n, false))
            }
            if !bTop && idx < count {
                let n = max(0, min(perRow + cornersTop, count - idx))
                var c = CGPoint(x: sel.midX, y: sel.minY - ext)
                if n == perRow + 1 { adjust(&c) }
                place(horizontal(c, n, false))
            }
            if !bLeft && idx < count {
                let n = max(0, min(perCol, count - idx))
                place(vertical(CGPoint(x: sel.minX - ext, y: sel.midY), n, true))
            }
            if idx < count {
                // expandSelection
                sel = sel.insetBy(dx: -ext, dy: -ext).intersection(screen)
            }
        }
        return Layout(frames: frames.map { $0 ?? .zero }, inside: inside)
    }
}

// MARK: - Pixels

// RGBA8 pixels of (part of) an image, for sampling (pixelate, grab color)
struct ShotPixels {
    let width: Int, height: Int
    var data: [UInt8]

    init(width: Int, height: Int, data: [UInt8]) {
        self.width = width; self.height = height; self.data = data
    }
    // the whole image (top row first)
    init?(_ image: CGImage) {
        let w = image.width, h = image.height
        guard w > 0, h > 0 else { return nil }
        var buf = [UInt8](repeating: 0, count: w * h * 4)
        let ok = buf.withUnsafeMutableBytes { raw -> Bool in
            guard let ctx = CGContext(data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8,
                                      bytesPerRow: w * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        guard ok else { return nil }
        self.init(width: w, height: h, data: buf)
    }
    // back to an image (row 0 = the top)
    var image: CGImage? {
        guard let provider = CGDataProvider(data: Data(data) as CFData) else { return nil }
        return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                       space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }
    func color(_ x: Int, _ y: Int) -> ShotColor {
        let cx = max(0, min(width - 1, x)), cy = max(0, min(height - 1, y))
        let i = (cy * width + cx) * 4
        return ShotColor(r: Double(data[i]) / 255, g: Double(data[i + 1]) / 255, b: Double(data[i + 2]) / 255)
    }
}

enum ShotPixelate {
    // Flameshot's block resolution: rect × 0.5 / (size + 1) blocks
    static func grid(_ r: CGRect, size: Int) -> (cols: Int, rows: Int) {
        let f = 0.5 / Double(max(1, size + 1))
        return (max(1, Int((Double(r.width) * f).rounded())), max(1, Int((Double(r.height) * f).rounded())))
    }

    // SECURE pixelate: every block is built ONLY from colors sampled on the
    // rect's outer fringe (a band 1-4 px outside each edge, averaged over the
    // block's span, smoothed across neighbours) + a little noise,
    // interpolated across — the hidden pixels never reach the output, so
    // nothing can be recovered. `px` = the rect in the pixel space of
    // `pixels`; `scale` = pixels per point (the grid is in points).
    static func secureBlocks(_ pixels: ShotPixels, px: CGRect, size: Int, scale: CGFloat = 1) -> [[ShotColor]] {
        let s = max(1, scale)
        let (cols, rows) = grid(CGRect(x: 0, y: 0, width: px.width / s, height: px.height / s), size: size)
        let x0 = Int(px.minX.rounded(.down)), y0 = Int(px.minY.rounded(.down))
        let x1 = Int(px.maxX.rounded(.up)), y1 = Int(px.maxY.rounded(.up))
        let band = 1...4
        func inX(_ x: Int) -> Bool { x >= 0 && x < pixels.width }
        func inY(_ y: Int) -> Bool { y >= 0 && y < pixels.height }
        // the mean of the band pixels (only those on the image, never inside the rect)
        func mean(_ pts: [(Int, Int)]) -> ShotColor? {
            var r = 0.0, g = 0.0, b = 0.0, n = 0.0
            for (x, y) in pts where inX(x) && inY(y) && !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                let c = pixels.color(x, y)
                r += c.r; g += c.g; b += c.b; n += 1
            }
            return n > 0 ? ShotColor(r: r / n, g: g / n, b: b / n) : nil
        }
        func span(_ i: Int, _ n: Int, _ a: Int, _ b: Int) -> ClosedRange<Int> {
            let lo = a + Int(Double(i) / Double(n) * Double(b - a))
            let hi = a + Int(Double(i + 1) / Double(n) * Double(b - a)) - 1
            return lo...max(lo, hi)
        }
        // up to 8 samples across each block's span (enough for a mean)
        func stepped(_ r: ClosedRange<Int>) -> [Int] {
            let st = max(1, r.count / 8)
            return Array(stride(from: r.lowerBound, through: r.upperBound, by: st))
        }
        var top: [ShotColor?] = [], bottom: [ShotColor?] = []
        for i in 0..<cols {
            let xs = stepped(span(i, cols, x0, x1))
            top.append(mean(xs.flatMap { x in band.map { (x, y0 - $0) } }))
            bottom.append(mean(xs.flatMap { x in band.map { (x, y1 - 1 + $0) } }))
        }
        var left: [ShotColor?] = [], right: [ShotColor?] = []
        for j in 0..<rows {
            let ys = stepped(span(j, rows, y0, y1))
            left.append(mean(ys.flatMap { y in band.map { (x0 - $0, y) } }))
            right.append(mean(ys.flatMap { y in band.map { (x1 - 1 + $0, y) } }))
        }
        // soften: each sample = the mean of itself and its neighbours
        func smooth(_ a: [ShotColor?]) -> [ShotColor?] {
            a.indices.map { k in
                let near = [k - 1, k, k + 1].filter { a.indices.contains($0) }.compactMap { a[$0] }
                guard !near.isEmpty else { return nil }
                let n = Double(near.count)
                return ShotColor(r: near.reduce(0) { $0 + $1.r } / n, g: near.reduce(0) { $0 + $1.g } / n,
                                 b: near.reduce(0) { $0 + $1.b } / n)
            }
        }
        top = smooth(top); bottom = smooth(bottom); left = smooth(left); right = smooth(right)
        func avg(_ cs: [(ShotColor, Double)]) -> ShotColor? {
            let w = cs.reduce(0) { $0 + $1.1 }
            guard w > 0 else { return nil }
            return ShotColor(r: cs.reduce(0) { $0 + $1.0.r * $1.1 } / w,
                             g: cs.reduce(0) { $0 + $1.0.g * $1.1 } / w,
                             b: cs.reduce(0) { $0 + $1.0.b * $1.1 } / w)
        }
        let gray = ShotColor(r: 0.5, g: 0.5, b: 0.5)
        var seed: UInt32 = UInt32(truncatingIfNeeded: x0 &* 73_856_093 ^ y0 &* 19_349_663) | 1
        func noise() -> Double {
            // xorshift: deterministic per rect, ±0.025
            seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5
            return (Double(seed % 1000) / 1000 - 0.5) * 0.05
        }
        var out: [[ShotColor]] = []
        for j in 0..<rows {
            var row: [ShotColor] = []
            let v = rows == 1 ? 0.5 : Double(j) / Double(rows - 1)
            for i in 0..<cols {
                let u = cols == 1 ? 0.5 : Double(i) / Double(cols - 1)
                var parts: [(ShotColor, Double)] = []
                if let c = top[i] { parts.append((c, 1 - v + 0.001)) }
                if let c = bottom[i] { parts.append((c, v + 0.001)) }
                if let c = left[j] { parts.append((c, 1 - u + 0.001)) }
                if let c = right[j] { parts.append((c, u + 0.001)) }
                var c = avg(parts) ?? gray
                let n = noise()
                c = ShotColor(r: max(0, min(1, c.r + n)), g: max(0, min(1, c.g + n)), b: max(0, min(1, c.b + n)))
                row.append(c)
            }
            out.append(row)
        }
        return out
    }
}

// MARK: - Arrow + text geometry

enum ShotArrow {
    static func headLength(_ size: Int) -> CGFloat { CGFloat(3 * size + 10) }
    static func headWidth(_ size: Int) -> CGFloat { CGFloat(2 * size + 6) }
    // tip, left and right base corners of the head pointing from a to b
    static func head(from a: CGPoint, to b: CGPoint, size: Int) -> (CGPoint, CGPoint, CGPoint, CGPoint) {
        let len = max(1, ShotGeom.dist(a, b))
        let ux = (b.x - a.x) / len, uy = (b.y - a.y) / len
        let hl = min(headLength(size), len), hw = headWidth(size) / 2
        let base = CGPoint(x: b.x - ux * hl, y: b.y - uy * hl)
        let l = CGPoint(x: base.x - uy * hw, y: base.y + ux * hw)
        let r = CGPoint(x: base.x + uy * hw, y: base.y - ux * hw)
        return (b, l, r, base)
    }
}

enum ShotText {
    static let padding: CGFloat = 5

    static func font(_ style: ShotTextStyle, size: CGFloat) -> CTFont {
        var f: CTFont = style.family.isEmpty
            ? CTFontCreateUIFontForLanguage(.system, size, nil) ?? CTFontCreateWithName("Helvetica" as CFString, size, nil)
            : CTFontCreateWithName(style.family as CFString, size, nil)
        var traits: CTFontSymbolicTraits = []
        if style.bold { traits.insert(.traitBold) }
        if style.italic { traits.insert(.traitItalic) }
        if !traits.isEmpty, let t = CTFontCreateCopyWithSymbolicTraits(f, size, nil, traits, traits) { f = t }
        return f
    }
    static func lines(_ o: ShotObject) -> [CTLine] {
        let f = font(o.style, size: o.fontSize)
        let attrs: [NSAttributedString.Key: Any] = [NSAttributedString.Key(kCTFontAttributeName as String): f,
                                                    NSAttributedString.Key(kCTForegroundColorAttributeName as String): o.color.cgColor]
        return o.text.components(separatedBy: "\n").map {
            CTLineCreateWithAttributedString(NSAttributedString(string: $0, attributes: attrs))
        }
    }
    static func metrics(_ o: ShotObject) -> (ascent: CGFloat, lineHeight: CGFloat) {
        let f = font(o.style, size: o.fontSize)
        let asc = CTFontGetAscent(f), desc = CTFontGetDescent(f), lead = CTFontGetLeading(f)
        return (asc, ceil(asc + desc + lead))
    }
    static func lineWidth(_ l: CTLine) -> CGFloat { CGFloat(CTLineGetTypographicBounds(l, nil, nil, nil)) }

    // the box grows with the text, 5 pt padding all round
    static func boxSize(_ o: ShotObject) -> CGSize {
        let ls = lines(o)
        let m = metrics(o)
        let w = max(m.lineHeight / 2, ls.map(lineWidth).max() ?? 0)
        return CGSize(width: ceil(w) + padding * 2, height: m.lineHeight * CGFloat(max(1, ls.count)) + padding * 2)
    }
}

// MARK: - Renderer

// The frozen display image + its pixel scale; pixelate block colors are
// cached per object geometry (drawn on every overlay redraw).
final class ShotCanvas {
    let base: CGImage
    let scale: CGFloat
    let size: CGSize               // the display in points
    private var pixelCache: ShotPixels?
    private var blockCache: [String: [[ShotColor]]] = [:]
    private var imageCache: [String: CGImage] = [:]

    init(base: CGImage, scale: CGFloat) {
        self.base = base
        self.scale = max(1, scale)
        size = CGSize(width: CGFloat(base.width) / self.scale, height: CGFloat(base.height) / self.scale)
    }
    var pixels: ShotPixels? {
        if pixelCache == nil { pixelCache = ShotPixels(base) }
        return pixelCache
    }
    func pxRect(_ r: CGRect) -> CGRect {
        CGRect(x: r.minX * scale, y: r.minY * scale, width: r.width * scale, height: r.height * scale).integral
    }
    // the color under a point (grab color)
    func color(at p: CGPoint) -> ShotColor? {
        pixels?.color(Int(p.x * scale), Int(p.y * scale))
    }
    func secureBlocks(_ r: CGRect, size: Int) -> [[ShotColor]] {
        let key = "\(r)|\(size)"
        if let b = blockCache[key] { return b }
        guard let px = pixels else { return [[ShotColor(r: 0.5, g: 0.5, b: 0.5)]] }
        let b = ShotPixelate.secureBlocks(px, px: pxRect(r), size: size, scale: scale)
        if blockCache.count > 64 { blockCache.removeAll() }
        blockCache[key] = b
        return b
    }
    // insecure: the real pixels, downscaled then upscaled (size ≤ 1: blur)
    func insecureImage(_ r: CGRect, size: Int) -> CGImage? {
        let key = "\(r)|\(size)"
        if let i = imageCache[key] { return i }
        guard r.width >= 1, r.height >= 1, let crop = base.cropping(to: pxRect(r)) else { return nil }
        var out: CGImage?
        if size <= 1 {
            let ci = CIImage(cgImage: crop).clampedToExtent()
                .applyingGaussianBlur(sigma: 10 * Double(scale)).cropped(to: CIImage(cgImage: crop).extent)
            out = CIContext().createCGImage(ci, from: ci.extent)
        } else {
            let (cols, rows) = ShotPixelate.grid(r, size: size)
            if let ctx = CGContext(data: nil, width: cols, height: rows, bitsPerComponent: 8, bytesPerRow: 0,
                                   space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                   bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
                ctx.interpolationQuality = .medium
                ctx.draw(crop, in: CGRect(x: 0, y: 0, width: cols, height: rows))
                out = ctx.makeImage()
            }
        }
        if imageCache.count > 32 { imageCache.removeAll() }
        imageCache[key] = out
        return out
    }
}

enum ShotRenderer {
    // draw an image upright at `r` in a top-left (flipped) context
    static func drawImage(_ img: CGImage, in r: CGRect, _ ctx: CGContext, smooth: Bool = true) {
        ctx.saveGState()
        ctx.interpolationQuality = smooth ? .high : .none
        ctx.translateBy(x: r.minX, y: r.maxY)
        ctx.scaleBy(x: 1, y: -1)
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: r.width, height: r.height))
        ctx.restoreGState()
    }

    // the frozen screen's `r` (points)
    static func drawBase(_ canvas: ShotCanvas, _ r: CGRect, _ ctx: CGContext) {
        let r = r.intersection(CGRect(origin: .zero, size: canvas.size))
        guard !r.isNull, r.width > 0, r.height > 0,
              let crop = canvas.base.cropping(to: canvas.pxRect(r)) else { return }
        // the cropped pixels' own extent (integral) in points
        let px = canvas.pxRect(r)
        let pr = CGRect(x: px.minX / canvas.scale, y: px.minY / canvas.scale,
                        width: CGFloat(crop.width) / canvas.scale, height: CGFloat(crop.height) / canvas.scale)
        drawImage(crop, in: pr, ctx, smooth: false)
    }

    static func draw(_ objects: [ShotObject], _ canvas: ShotCanvas, _ ctx: CGContext) {
        for o in objects { draw(o, canvas, ctx) }
    }

    static func draw(_ o: ShotObject, _ canvas: ShotCanvas, _ ctx: CGContext) {
        ctx.saveGState()
        defer { ctx.restoreGState() }
        ctx.setShouldAntialias(true)
        let col = o.color.cgColor
        ctx.setStrokeColor(col)
        ctx.setFillColor(col)
        ctx.setLineWidth(o.strokeWidth)
        ctx.setLineCap(.round)
        ctx.setLineJoin(.round)
        switch o.tool {
        case .pencil:
            stroke(o.points, ctx, width: o.strokeWidth)
        case .line:
            stroke([o.start, o.end], ctx)
        case .marker:
            ctx.setBlendMode(.multiply)
            ctx.setStrokeColor(o.color.with(alpha: 0.4).cgColor)
            ctx.setLineCap(.butt)
            stroke(o.points.count > 2 ? o.points : [o.start, o.end], ctx, width: o.strokeWidth)
        case .arrow:
            let (a, b) = o.reversed ? (o.end, o.start) : (o.start, o.end)
            guard ShotGeom.dist(a, b) > 0.5 else { stroke([a], ctx, width: o.strokeWidth); return }
            let (tip, l, r, base) = ShotArrow.head(from: a, to: b, size: o.size)
            if o.openArrow {
                stroke([a, b], ctx)
                stroke([l, tip, r], ctx)
            } else {
                stroke([a, base], ctx)
                ctx.setLineJoin(.miter)
                ctx.setLineWidth(1)
                ctx.beginPath()
                ctx.move(to: tip); ctx.addLine(to: l); ctx.addLine(to: r); ctx.closePath()
                ctx.drawPath(using: .fillStroke)
            }
        case .selection:
            ctx.setLineJoin(.miter)
            ctx.setLineCap(.square)
            ctx.stroke(o.rect)
        case .rectangle:
            let rr = min(CGFloat(o.size), min(o.rect.width, o.rect.height) / 2)
            ctx.addPath(CGPath(roundedRect: o.rect, cornerWidth: max(0, rr), cornerHeight: max(0, rr), transform: nil))
            ctx.fillPath()
        case .circle:
            ctx.strokeEllipse(in: o.rect)
        case .text:
            drawText(o, ctx)
        case .counter:
            drawCounter(o, ctx)
        case .pixelate:
            let r = o.rect.integral
            guard r.width >= 1, r.height >= 1 else { return }
            if o.secure {
                let blocks = canvas.secureBlocks(r, size: o.size)
                let rows = blocks.count, cols = blocks.first?.count ?? 1
                ctx.setShouldAntialias(false)
                for (j, row) in blocks.enumerated() {
                    for (i, c) in row.enumerated() {
                        let x0 = r.minX + r.width * CGFloat(i) / CGFloat(cols)
                        let x1 = r.minX + r.width * CGFloat(i + 1) / CGFloat(cols)
                        let y0 = r.minY + r.height * CGFloat(j) / CGFloat(rows)
                        let y1 = r.minY + r.height * CGFloat(j + 1) / CGFloat(rows)
                        ctx.setFillColor(c.cgColor)
                        ctx.fill(CGRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0))
                    }
                }
            } else if let img = canvas.insecureImage(r, size: o.size) {
                drawImage(img, in: r, ctx, smooth: o.size <= 1)
            }
        case .invert:
            ctx.setBlendMode(.difference)
            ctx.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1))
            ctx.fill(o.rect)
        default:
            break
        }
    }

    private static func stroke(_ pts: [CGPoint], _ ctx: CGContext, width w: CGFloat = 0) {
        guard let f = pts.first else { return }
        if pts.count == 1 {
            // a click: a dot of the stroke width
            ctx.fillEllipse(in: CGRect(x: f.x - w / 2, y: f.y - w / 2, width: w, height: w))
            return
        }
        ctx.beginPath()
        ctx.move(to: f)
        for p in pts.dropFirst() { ctx.addLine(to: p) }
        ctx.strokePath()
    }

    static func drawText(_ o: ShotObject, _ ctx: CGContext) {
        let ls = ShotText.lines(o)
        let m = ShotText.metrics(o)
        let box = CGRect(origin: o.start, size: ShotText.boxSize(o))
        let inner = box.width - ShotText.padding * 2
        ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        for (i, l) in ls.enumerated() {
            let w = ShotText.lineWidth(l)
            let dx: CGFloat = o.style.align == 1 ? (inner - w) / 2 : o.style.align == 2 ? inner - w : 0
            let x = box.minX + ShotText.padding + dx
            let y = box.minY + ShotText.padding + m.ascent + CGFloat(i) * m.lineHeight
            ctx.textPosition = CGPoint(x: x, y: y)
            CTLineDraw(l, ctx)
            if (o.style.underline || o.style.strike) && w > 0 {
                let t = max(1, o.fontSize / 14)
                ctx.setFillColor(o.color.cgColor)
                if o.style.underline { ctx.fill(CGRect(x: x, y: y + t * 2, width: w, height: t)) }
                if o.style.strike { ctx.fill(CGRect(x: x, y: y - m.ascent * 0.32, width: w, height: t)) }
            }
        }
    }

    static func drawCounter(_ o: ShotObject, _ ctx: CGContext) {
        let c = o.start, r = o.counterRadius
        let contrast = o.color.isDark ? ShotColor.white : ShotColor.black
        ctx.setFillColor(o.color.cgColor)
        // the tail: a tapered wedge from the bubble toward the drag point
        if o.points.count > 1, ShotGeom.dist(c, o.end) > r {
            let d = ShotGeom.dist(c, o.end)
            let ux = (o.end.x - c.x) / d, uy = (o.end.y - c.y) / d
            let half = r * 0.6
            ctx.beginPath()
            ctx.move(to: CGPoint(x: c.x - uy * half, y: c.y + ux * half))
            ctx.addLine(to: o.end)
            ctx.addLine(to: CGPoint(x: c.x + uy * half, y: c.y - ux * half))
            ctx.closePath()
            ctx.fillPath()
        }
        let circle = CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r)
        ctx.fillEllipse(in: circle)
        if o.outline {
            ctx.setStrokeColor(contrast.with(alpha: 0.9).cgColor)
            ctx.setLineWidth(1)
            ctx.strokeEllipse(in: circle.insetBy(dx: 0.5, dy: 0.5))
        }
        var t = ShotObject(tool: .text, points: [.zero], color: contrast, size: 0)
        t.style.bold = true
        let font = ShotText.font(t.style, size: r * (o.number > 99 ? 0.75 : 1.0))
        let attrs: [NSAttributedString.Key: Any] = [NSAttributedString.Key(kCTFontAttributeName as String): font,
                                                    NSAttributedString.Key(kCTForegroundColorAttributeName as String): contrast.cgColor]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: "\(o.number)", attributes: attrs))
        let b = CTLineGetImageBounds(line, nil)
        ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        ctx.textPosition = CGPoint(x: c.x - b.midX, y: c.y + b.midY)
        CTLineDraw(line, ctx)
    }

    // THE output: the frozen pixels under `crop` (points) at the display's
    // native scale + every object, clipped to the selection
    static func render(_ canvas: ShotCanvas, crop: CGRect, objects: [ShotObject]) -> CGImage? {
        let crop = crop.intersection(CGRect(origin: .zero, size: canvas.size))
        guard !crop.isNull, crop.width >= 1, crop.height >= 1 else { return nil }
        let w = Int((crop.width * canvas.scale).rounded()), h = Int((crop.height * canvas.scale).rounded())
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        // top-left points over the crop
        ctx.translateBy(x: 0, y: CGFloat(h))
        ctx.scaleBy(x: 1, y: -1)
        ctx.scaleBy(x: canvas.scale, y: canvas.scale)
        ctx.translateBy(x: -crop.minX, y: -crop.minY)
        ctx.clip(to: crop)
        drawBase(canvas, crop, ctx)
        draw(objects, canvas, ctx)
        return ctx.makeImage()
    }
}

// MARK: - Files

enum ShotFiles {
    // strftime (`%F_%H-%M` → 2026-10-02_14-05); "/" is not allowed in a name
    static func expand(_ pattern: String, date: Date = Date()) -> String {
        var t = time_t(date.timeIntervalSince1970)
        var tmv = tm()
        localtime_r(&t, &tmv)
        var buf = [Int8](repeating: 0, count: 512)
        let n = strftime(&buf, buf.count, pattern.isEmpty ? "%F_%H-%M" : pattern, &tmv)
        let s = n > 0 ? String(cString: buf) : "screenshot"
        return s.replacingOccurrences(of: "/", with: "-")
    }
    // dir/name.ext, else "name 2.ext", "name 3.ext" … (FileDrag's keep-both)
    static func uniquePath(dir: String, name: String, ext: String,
                           exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> String {
        let d = dir.hasSuffix("/") ? String(dir.dropLast()) : dir
        let e = ext.isEmpty ? "" : "." + ext
        var p = "\(d)/\(name)\(e)"
        var n = 2
        while exists(p) && n < 10_000 {
            p = "\(d)/\(name) \(n)\(e)"
            n += 1
        }
        return p
    }
    // a `-p` / save target: a directory → the pattern inside it; a file
    // path keeps its name; the extension picks the format
    static func target(_ path: String, pattern: String, format: String, date: Date = Date(),
                       isDir: (String) -> Bool = { p in
                           var d: ObjCBool = false
                           return FileManager.default.fileExists(atPath: p, isDirectory: &d) && d.boolValue
                       },
                       exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> String {
        let p = (path as NSString).expandingTildeInPath
        if isDir(p) || p.hasSuffix("/") {
            return uniquePath(dir: p, name: expand(pattern, date: date), ext: format, exists: exists)
        }
        return (p as NSString).pathExtension.isEmpty ? p + "." + format : p
    }
}

// MARK: - CLI

// `workspace-switcher screenshot [gui|full|screen] [flags]` (Flameshot's)
struct ShotArgs: Equatable {
    enum Mode: String { case gui, text, full, screen }   // text = gui in Copy Text mode
    var mode = Mode.gui
    var path: String?
    var clipboard = false
    var delayMs = 0
    var region: String?
    var lastRegion = false
    var acceptOnSelect = false
    var pin = false
    var raw = false
    var printGeometry = false
    var screenNumber: Int?

    var isOverlay: Bool { mode == .gui || mode == .text }

    // the socket reply carries a result (-r / -g)
    var wantsReply: Bool { raw || printGeometry }

    struct Problem: Error, Equatable { let message: String }

    static func parse(_ words: [String]) -> Result<ShotArgs, Problem> {
        var a = ShotArgs()
        var i = 0
        func value(_ flag: String) -> String? {
            guard i + 1 < words.count else { return nil }
            i += 1
            return words[i]
        }
        if let f = words.first, let m = Mode(rawValue: f) { a.mode = m; i = 1 }
        while i < words.count {
            let w = words[i]
            switch w {
            case "-p", "--path":
                guard let v = value(w) else { return .failure(Problem(message: "\(w) needs a path")) }
                a.path = v
            case "-c", "--clipboard": a.clipboard = true
            case "-d", "--delay":
                guard let v = value(w), let n = Int(v), n >= 0 else { return .failure(Problem(message: "\(w) needs milliseconds")) }
                a.delayMs = n
            case "--region":
                guard let v = value(w), parseRegion(v) != nil || v.hasPrefix("screen") else {
                    return .failure(Problem(message: "--region WxH+X+Y | screenN"))
                }
                a.region = v
            case "--last-region": a.lastRegion = true
            case "-s", "--accept-on-select": a.acceptOnSelect = true
            case "--pin": a.pin = true
            case "-r", "--raw": a.raw = true
            case "-g", "--print-geometry": a.printGeometry = true
            case "-n", "--number":
                guard let v = value(w), let n = Int(v), n >= 0 else { return .failure(Problem(message: "-n needs a screen number")) }
                a.screenNumber = n
            default:
                return .failure(Problem(message: "unknown option \(w)"))
            }
            i += 1
        }
        return .success(a)
    }

    // WxH+X+Y (global points, top-left origin)
    static func parseRegion(_ s: String) -> CGRect? {
        let scanner = Scanner(string: s)
        guard let w = scanner.scanDouble(), scanner.scanString("x") != nil,
              let h = scanner.scanDouble() else { return nil }
        var x = 0.0, y = 0.0
        if scanner.scanString("+") != nil {
            guard let v = scanner.scanDouble() else { return nil }
            x = v
            guard scanner.scanString("+") != nil, let v2 = scanner.scanDouble() else { return nil }
            y = v2
        }
        guard scanner.isAtEnd, w > 0, h > 0 else { return nil }
        return CGRect(x: x, y: y, width: w, height: h)
    }
}

// MARK: - Persistent state

// ~/.cache/workspace-switcher/screenshot-state.json: per-tool sizes, the
// draw color, text style, the last accepted region (save-last-region)
struct ShotState: Codable {
    struct Region: Codable, Equatable {
        var display: UInt32
        var x: Double, y: Double, w: Double, h: Double
    }
    var sizes: [String: Int] = [:]
    var color: String?
    var style = ShotTextStyle()
    var lastRegion: Region?
    var gridSize = 10
    var grid = false

    func size(_ t: ShotTool) -> Int { sizes[t.rawValue] ?? t.defaultSize }

    static func load(_ path: String) -> ShotState {
        guard let d = FileManager.default.contents(atPath: path),
              let s = try? JSONDecoder().decode(ShotState.self, from: d) else { return ShotState() }
        return s
    }
    func save(_ path: String) {
        try? FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent,
                                                 withIntermediateDirectories: true)
        if let d = try? JSONEncoder().encode(self) { try? d.write(to: URL(fileURLWithPath: path), options: .atomic) }
    }
}
