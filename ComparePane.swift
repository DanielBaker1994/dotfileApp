import AppKit

final class CompareSession {
    private static var nextID = 1
    let id: Int
    var model: TextCompare
    var path: [CompareSide: String] = [:]
    var title: [CompareSide: String] = [:]
    var disk: [CompareSide: Data] = [:]
    var binary: [CompareSide: Data] = [:]
    var tooLarge: [CompareSide: Bool] = [:]
    var git = false
    var cleanDepth: [CompareSide: Int] = [.left: 0, .right: 0]
    var filter: CompareFilter = .all
    private(set) var visible: [Int]?
    var cursor = 0
    var anchor: Int?
    var focus: CompareSide = .left
    var col = 0
    var scrollY: CGFloat = 0
    var version = 0
    var waiters: [() -> Void] = []
    var changedOnDisk: [CompareSide: Bool] = [:]
    var watchers: [CompareSide: DispatchSourceFileSystemObject] = [:]
    var status = ""
    var folder: FolderSession?
    var recovered = false
    var recoveryVersion: [CompareSide: Int] = [:]

    init(model: TextCompare) {
        self.model = model
        id = Self.nextID
        Self.nextID += 1
    }

    private func edits(_ s: CompareSide) -> Int { model.undoStack.reduce(0) { $0 + ($1.0 == s ? 1 : 0) } }
    func dirty(_ s: CompareSide) -> Bool { edits(s) != cleanDepth[s] }
    var isDirty: Bool { dirty(.left) || dirty(.right) }
    func markClean(_ s: CompareSide) { cleanDepth[s] = edits(s) }
    func willEdit(_ s: CompareSide) { if edits(s) < cleanDepth[s] ?? 0 { cleanDepth[s] = -1 } }

    var isBinary: Bool { !binary.isEmpty || tooLarge.values.contains(true) }

    var displayCount: Int { visible?.count ?? model.rows.count }
    func modelRow(_ d: Int) -> Int {
        guard let v = visible else { return d }
        return v.indices.contains(d) ? v[d] : (v.last ?? 0)
    }
    func displayRow(_ m: Int) -> Int {
        guard let v = visible else { return m }
        var lo = 0, hi = v.count
        while lo < hi {
            let mid = (lo + hi) / 2
            if v[mid] < m { lo = mid + 1 } else { hi = mid }
        }
        return min(lo, max(0, v.count - 1))
    }

    func refresh(context: Int) {
        version += 1
        visible = model.visibleRows(filter, context: context)
        cursor = min(max(0, cursor), max(0, displayCount - 1))
        if let a = anchor, a >= displayCount { anchor = nil }
    }

    func name(_ s: CompareSide) -> String {
        if let f = folder { return ((s == .left ? f.leftRoot : f.rightRoot) as NSString).lastPathComponent + "/" }
        if let t = title[s], !t.isEmpty { return (t as NSString).lastPathComponent }
        if let p = path[s] { return (p as NSString).lastPathComponent }
        return model.side(s).lines.isEmpty ? "(empty)" : "(pasted)"
    }

    var label: String {
        if let f = folder { return (git ? "(git) " : "") + f.label }
        let l = name(.left), r = name(.right)
        return (git ? "(git) " : "") + (l == r ? l : "\(l) ⇆ \(r)")
    }

    func stopWatching() {
        for w in watchers.values { w.cancel() }
        watchers = [:]
    }
}

protocol ComparePaneHost: AnyObject {
    var paneSession: CompareSession? { get }
    var paneColors: PopupColors { get }
    var paneGutterArrows: String { get }
    var paneTabWidth: Int { get }
    var paneShowWhitespace: Bool { get }
    var paneAlignPick: (side: CompareSide, line: Int)? { get }
    func paneClicked(row: Int, side: CompareSide, col: Int, clicks: Int, shift: Bool)
    func paneDragged(to row: Int)
    func paneGutterCopy(section: Int, from: CompareSide)
    func paneMenu(row: Int, side: CompareSide) -> NSMenu?
    func paneDrop(side: CompareSide, urls: [URL], text: String?) -> Bool
    func paneScrolled()
}

final class ComparePaneView: NSView {
    weak var host: ComparePaneHost?
    private(set) var font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private(set) var rowH: CGFloat = 18
    private(set) var charW: CGFloat = 8
    private var ascent: CGFloat = 12
    var hOffset: CGFloat = 0 { didSet { if hOffset != oldValue { needsDisplay = true } } }
    var hoverRow: Int? { didSet { if hoverRow != oldValue { needsDisplay = true } } }
    private var marksCache: [Int: (left: [CharDiff.Mark], right: [CharDiff.Mark])] = [:]
    private var marksVersion = -1
    private var para = NSMutableParagraphStyle()
    var gutterW: CGFloat { 34 }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var isOpaque: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL, .string])
        setFont(font)
    }
    required init?(coder: NSCoder) { fatalError() }

    func setFont(_ f: NSFont) {
        font = f
        ascent = f.ascender
        let natural = ceil(f.ascender - f.descender + f.leading)
        rowH = natural + 4
        charW = ("M" as NSString).size(withAttributes: [.font: f]).width
        let p = NSMutableParagraphStyle()
        p.tabStops = []
        p.defaultTabInterval = charW * CGFloat(host?.paneTabWidth ?? 4)
        p.lineBreakMode = .byClipping
        para = p
        needsDisplay = true
    }

    var paneW: CGFloat { floor((bounds.width - gutterW) / 2) }
    private var lineNoW: CGFloat {
        guard let s = host?.paneSession else { return charW * 3 + 12 }
        let n = max(s.model.left.lines.count, s.model.right.lines.count, 99)
        return CGFloat(String(n).count) * charW + 14
    }
    func paneX(_ side: CompareSide) -> CGFloat { side == .left ? 0 : paneW + gutterW }
    func textRect(_ side: CompareSide, row d: Int, rows: Int = 1) -> NSRect {
        let x = paneX(side) + lineNoW + 6
        return NSRect(x: x, y: CGFloat(d) * rowH, width: paneW - lineNoW - 10, height: rowH * CGFloat(rows))
    }
    func side(at x: CGFloat) -> CompareSide { x < paneW + gutterW / 2 ? .left : .right }
    func row(at y: CGFloat) -> Int { max(0, Int(y / rowH)) }
    func column(at x: CGFloat, side: CompareSide) -> Int {
        max(0, Int(((x - textRect(side, row: 0).minX + hOffset) / charW).rounded(.down)))
    }
    var docHeight: CGFloat { CGFloat(host?.paneSession?.displayCount ?? 0) * rowH }

    func invalidateMarks() { marksCache = [:] }

    private func marks(_ m: Int, _ s: CompareSession) -> (left: [CharDiff.Mark], right: [CharDiff.Mark]) {
        if marksVersion != s.version { marksCache = [:]; marksVersion = s.version }
        if let c = marksCache[m] { return c }
        let row = s.model.rows[m]
        var out: (left: [CharDiff.Mark], right: [CharDiff.Mark]) = ([], [])
        if row.kind == .changed, row.l >= 0, row.r >= 0 {
            let a = s.model.left.lines[Int(row.l)], b = s.model.right.lines[Int(row.r)]
            if a.utf16.count < 2000 && b.utf16.count < 2000 {
                out = CharDiff.marks(a, b, s.model.importance)
            }
        }
        marksCache[m] = out
        return out
    }

    func attributed(_ text: String, color: NSColor, marks: [CharDiff.Mark], c: PopupColors,
                    bg: NSColor) -> NSMutableAttributedString {
        let a = NSMutableAttributedString(string: text, attributes: [.font: font, .foregroundColor: c.ensure(color, on: bg),
                                                                     .paragraphStyle: para])
        let len = a.length
        for mk in marks {
            let r = NSIntersectionRange(mk.range, NSRange(location: 0, length: len))
            guard r.length > 0 else { continue }
            let hue = c.tone(mk.important ? .danger : .info)
            let markBg = c.over(hue, 0.22, on: bg)
            a.addAttributes([.backgroundColor: markBg,
                             .foregroundColor: c.ensure(color, on: markBg),
                             .underlineStyle: NSUnderlineStyle.thick.rawValue,
                             .underlineColor: c.ensure(hue, on: markBg, 3)], range: r)
        }
        return a
    }

    private func dimWhitespace(_ a: NSMutableAttributedString, original: String, c: PopupColors) {
        let u = Array(original.utf16)
        for (i, ch) in u.enumerated() where ch == 32 {
            a.addAttribute(.foregroundColor, value: c.dim.withAlphaComponent(0.6), range: NSRange(location: i, length: 1))
        }
    }

    private func drawTabs(_ text: String, x: CGFloat, y: CGFloat, c: PopupColors) {
        guard text.contains("\t") else { return }
        let tw = max(1, host?.paneTabWidth ?? 4)
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.dim.withAlphaComponent(0.6)]
        var col = 0
        for ch in text {
            if ch == "\t" {
                ("→" as NSString).draw(at: NSPoint(x: x + CGFloat(col) * charW, y: y), withAttributes: attrs)
                col = (col / tw + 1) * tw
            } else {
                col += 1
            }
        }
    }

    override func draw(_ dirty: NSRect) {
        guard let host, let s = host.paneSession else { return }
        let c = host.paneColors
        c.base.setFill()
        dirty.fill()
        let gx = paneW
        c.mantle.setFill()
        NSRect(x: gx, y: dirty.minY, width: gutterW, height: dirty.height).fill()
        let n = s.displayCount
        guard n > 0 else { return }
        let first = max(0, Int(dirty.minY / rowH)), last = min(n - 1, Int(dirty.maxY / rowH))
        if first <= last {
            let lo = min(s.cursor, s.anchor ?? s.cursor), hi = max(s.cursor, s.anchor ?? s.cursor)
            for d in first...last { drawRow(d, s, c, selected: s.anchor != nil && d >= lo && d <= hi) }
        }
        c.hairline.setFill()
        NSRect(x: gx, y: dirty.minY, width: 1, height: dirty.height).fill()
        NSRect(x: gx + gutterW - 1, y: dirty.minY, width: 1, height: dirty.height).fill()
    }

    private func drawRow(_ d: Int, _ s: CompareSession, _ c: PopupColors, selected: Bool) {
        let m = s.modelRow(d)
        guard s.model.rows.indices.contains(m) else { return }
        let row = s.model.rows[m]
        let y = CGFloat(d) * rowH
        let diff = s.model.isDiff(row)
        let hue = c.tone(row.important ? .danger : .info)
        let mk = diff && row.kind == .changed ? marks(m, s) : ([], [])
        for side in [CompareSide.left, .right] {
            let px = paneX(side)
            let rect = NSRect(x: px, y: y, width: paneW, height: rowH)
            let line = row.line(side)
            if line < 0 {
                c.mantle.setFill()
                rect.fill()
                NSGraphicsContext.saveGraphicsState()
                rect.clip()
                let hatch = NSBezierPath()
                var x = px - rowH
                while x < px + paneW {
                    hatch.move(to: NSPoint(x: x, y: y + rowH))
                    hatch.line(to: NSPoint(x: x + rowH, y: y))
                    x += 7
                }
                c.text.withAlphaComponent(c.isLight ? 0.10 : 0.07).setStroke()
                hatch.lineWidth = 1
                hatch.stroke()
                NSGraphicsContext.restoreGraphicsState()
            } else if diff {
                hue.withAlphaComponent(row.important ? 0.15 : 0.13).setFill()
                rect.fill()
            }
            var under = diff ? c.over(hue, row.important ? 0.15 : 0.13, on: c.base) : c.base
            if d == s.cursor {
                let on = side == s.focus
                c.highlight.withAlphaComponent(on ? 0.55 : 0.22).setFill()
                rect.fill()
                under = c.over(c.highlight, on ? 0.55 : 0.22, on: under)
                if on {
                    c.accentOn.setFill()
                    NSRect(x: px, y: y, width: 3, height: rowH).fill()
                }
            } else if selected && side == s.focus {
                c.accentOn.withAlphaComponent(0.16).setFill()
                rect.fill()
                under = c.over(c.accentOn, 0.16, on: under)
            }
            guard line >= 0 else { continue }
            let num = String(line + 1) as NSString
            let numAttrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.ensure(c.dim, on: under)]
            let nw = num.size(withAttributes: numAttrs).width
            num.draw(at: NSPoint(x: px + lineNoW - nw - 4, y: y + 2), withAttributes: numAttrs)
            let tr = textRect(side, row: d)
            let text = side == .left ? s.model.left.lines[line] : s.model.right.lines[line]
            guard !text.isEmpty else { continue }
            let showWS = host?.paneShowWhitespace ?? false
            let a = attributed(showWS ? text.replacingOccurrences(of: " ", with: "·") : text, color: c.text,
                               marks: side == .left ? mk.0 : mk.1, c: c, bg: under)
            if showWS { dimWhitespace(a, original: text, c: c) }
            NSGraphicsContext.saveGraphicsState()
            NSRect(x: tr.minX - 2, y: y, width: tr.width + 2, height: rowH).clip()
            a.draw(at: NSPoint(x: tr.minX - hOffset, y: y + 2))
            if showWS { drawTabs(text, x: tr.minX - hOffset, y: y + 2, c: c) }
            NSGraphicsContext.restoreGraphicsState()
        }
        if s.model.isAnchor(row: m) {
            c.tone(.accent2).withAlphaComponent(0.8).setFill()
            NSRect(x: 0, y: y, width: bounds.width, height: 1.5).fill()
            let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 10, weight: .bold), .foregroundColor: c.tone(.accent2)]
            ("⚓︎" as NSString).draw(at: NSPoint(x: paneW + gutterW / 2 - 5, y: y + 2), withAttributes: attrs)
        }
        if let pick = host?.paneAlignPick, row.line(pick.side) == pick.line {
            let r = NSRect(x: paneX(pick.side) + 1, y: y + 1, width: paneW - 2, height: rowH - 2)
            let path = NSBezierPath(roundedRect: r, xRadius: 3, yRadius: 3)
            path.lineWidth = 1.5
            path.setLineDash([4, 3], count: 2, phase: 0)
            c.tone(.accent2).setStroke()
            path.stroke()
        }
        if diff, let si = s.model.section(at: m), s.model.sections[si].rows.lowerBound == m || d == 0 {
            let mode = host?.paneGutterArrows ?? "hover"
            let inSection = s.model.section(at: s.modelRow(s.cursor)) == si
                || hoverRow.map { s.model.section(at: s.modelRow($0)) == si } == true
            if mode == "always" || (mode == "hover" && inSection) {
                let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12, weight: .bold),
                                                            .foregroundColor: c.accentOn]
                ("→" as NSString).draw(at: NSPoint(x: paneW + 3, y: y + 1), withAttributes: attrs)
                ("←" as NSString).draw(at: NSPoint(x: paneW + gutterW - 15, y: y + 1), withAttributes: attrs)
            } else {
                c.ensure(hue, on: c.mantle, 3).setFill()
                NSRect(x: paneW + gutterW / 2 - 2, y: y + 4, width: 4, height: rowH - 8).fill()
            }
        } else if diff {
            c.ensure(hue, on: c.mantle, 3).setFill()
            NSRect(x: paneW + gutterW / 2 - 2, y: y + 4, width: 4, height: rowH - 8).fill()
        }
    }

    private var dragging = false

    override func mouseDown(with e: NSEvent) {
        window?.makeFirstResponder(self)
        guard let host, let s = host.paneSession, s.displayCount > 0 else { return }
        let p = convert(e.locationInWindow, from: nil)
        let d = min(row(at: p.y), s.displayCount - 1)
        if p.x >= paneW && p.x < paneW + gutterW, let si = s.model.section(at: s.modelRow(d)) {
            host.paneGutterCopy(section: si, from: p.x < paneW + gutterW / 2 ? .left : .right)
            return
        }
        let side = side(at: p.x)
        host.paneClicked(row: d, side: side, col: column(at: p.x, side: side), clicks: e.clickCount,
                         shift: e.modifierFlags.contains(.shift))
        dragging = true
    }

    override func mouseDragged(with e: NSEvent) {
        guard dragging, let host, let s = host.paneSession, s.displayCount > 0 else { return }
        autoscroll(with: e)
        let p = convert(e.locationInWindow, from: nil)
        host.paneDragged(to: min(row(at: p.y), s.displayCount - 1))
    }

    override func mouseUp(with e: NSEvent) { dragging = false }

    override func rightMouseDown(with e: NSEvent) {
        guard let host, let s = host.paneSession else { return }
        let p = convert(e.locationInWindow, from: nil)
        let d = s.displayCount == 0 ? 0 : min(row(at: p.y), s.displayCount - 1)
        let side = side(at: p.x)
        if s.displayCount > 0 { host.paneClicked(row: d, side: side, col: column(at: p.x, side: side), clicks: 1, shift: false) }
        if let menu = host.paneMenu(row: d, side: side) {
            NSMenu.popUpContextMenu(menu, with: e, for: self)
        }
    }

    override func scrollWheel(with e: NSEvent) {
        if abs(e.scrollingDeltaX) > abs(e.scrollingDeltaY) {
            hOffset = max(0, hOffset - e.scrollingDeltaX * (e.hasPreciseScrollingDeltas ? 1 : charW))
            return
        }
        super.scrollWheel(with: e)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }
    override func mouseMoved(with e: NSEvent) {
        hoverRow = row(at: convert(e.locationInWindow, from: nil).y)
    }
    override func mouseExited(with e: NSEvent) { hoverRow = nil }

    override func draggingEntered(_ info: NSDraggingInfo) -> NSDragOperation { .copy }
    override func draggingUpdated(_ info: NSDraggingInfo) -> NSDragOperation { .copy }
    override func performDragOperation(_ info: NSDraggingInfo) -> Bool {
        let p = convert(info.draggingLocation, from: nil)
        let pb = info.draggingPasteboard
        let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        return host?.paneDrop(side: side(at: p.x), urls: urls, text: urls.isEmpty ? pb.string(forType: .string) : nil) ?? false
    }
}

final class CompareThumbnail: NSView {
    weak var host: ComparePaneHost?
    var visibleFraction: (top: CGFloat, height: CGFloat) = (0, 1) { didSet { needsDisplay = true } }
    var onScroll: ((CGFloat) -> Void)?
    private var cache: (key: String, image: CGImage)?
    override var isFlipped: Bool { true }

    private func image(_ s: CompareSession, _ c: PopupColors, height: Int) -> CGImage? {
        let key = "\(s.id).\(s.version).\(height).\(s.model.ignoreUnimportant)"
        if let cache, cache.key == key { return cache.image }
        let n = s.displayCount
        guard n > 0, height > 0 else { return nil }
        var level = [UInt8](repeating: 0, count: height)
        for d in 0..<n {
            let row = s.model.rows[s.modelRow(d)]
            guard s.model.isDiff(row) else { continue }
            let y0 = d * height / n, y1 = max(y0 + 1, (d + 1) * height / n)
            let v: UInt8 = row.important ? 2 : 1
            for y in y0..<min(y1, height) where level[y] < v { level[y] = v }
        }
        func px(_ col: NSColor) -> UInt32 {
            let k = col.usingColorSpace(.sRGB) ?? col
            return UInt32(k.alphaComponent * 255) << 24 | UInt32(k.blueComponent * 255) << 16
                | UInt32(k.greenComponent * 255) << 8 | UInt32(k.redComponent * 255)
        }
        let colors: [UInt32] = [0, px(c.tone(.info)), px(c.tone(.danger))]
        var pixels = level.map { colors[Int($0)] }
        let img = pixels.withUnsafeMutableBytes { raw -> CGImage? in
            guard let ctx = CGContext(data: raw.baseAddress, width: 1, height: height, bitsPerComponent: 8, bytesPerRow: 4,
                                      space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
            return ctx.makeImage()
        }
        if let img { cache = (key, img) }
        return img
    }

    override func draw(_ dirty: NSRect) {
        guard let host else { return }
        let c = host.paneColors
        c.mantle.setFill()
        bounds.fill()
        guard let s = host.paneSession, let ctx = NSGraphicsContext.current?.cgContext else { return }
        if let img = image(s, c, height: max(1, Int(bounds.height))) {
            ctx.saveGState()
            ctx.interpolationQuality = .none
            ctx.translateBy(x: 0, y: bounds.height)
            ctx.scaleBy(x: 1, y: -1)
            ctx.draw(img, in: CGRect(x: 2, y: 0, width: bounds.width - 4, height: bounds.height))
            ctx.restoreGState()
        }
        let r = NSRect(x: 0.5, y: visibleFraction.top * bounds.height + 0.5, width: bounds.width - 1,
                       height: max(4, visibleFraction.height * bounds.height) - 1)
        c.text.withAlphaComponent(0.10).setFill()
        r.fill()
        c.accentOn.withAlphaComponent(0.8).setStroke()
        NSBezierPath(rect: r).stroke()
    }

    override func mouseDown(with e: NSEvent) { jump(e) }
    override func mouseDragged(with e: NSEvent) { jump(e) }
    private func jump(_ e: NSEvent) {
        let y = convert(e.locationInWindow, from: nil).y
        onScroll?(max(0, min(1, y / max(1, bounds.height))))
    }
}

final class CompareDetails: NSView {
    weak var pane: ComparePaneView?
    weak var host: ComparePaneHost?
    override var isFlipped: Bool { true }
    override func draw(_ dirty: NSRect) {
        guard let host, let pane else { return }
        let c = host.paneColors
        c.mantle.setFill()
        bounds.fill()
        c.hairline.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
        guard let s = host.paneSession, s.displayCount > 0 else { return }
        let m = s.modelRow(s.cursor)
        guard s.model.rows.indices.contains(m) else { return }
        let row = s.model.rows[m]
        var mk: (left: [CharDiff.Mark], right: [CharDiff.Mark]) = ([], [])
        if row.kind == .changed, row.l >= 0, row.r >= 0 {
            mk = CharDiff.marks(s.model.left.lines[Int(row.l)], s.model.right.lines[Int(row.r)], s.model.importance)
        }
        let labelAttrs: [NSAttributedString.Key: Any] = [.font: pane.font, .foregroundColor: c.accentOn]
        for (i, side) in [CompareSide.left, .right].enumerated() {
            let y = 5 + CGFloat(i) * pane.rowH
            let line = row.line(side)
            (side == .left ? "L" : "R" as NSString).draw(at: NSPoint(x: 10, y: y), withAttributes: labelAttrs)
            let num = line >= 0 ? String(line + 1) : "—"
            (num as NSString).draw(at: NSPoint(x: 26, y: y), withAttributes: [.font: pane.font,
                                                                             .foregroundColor: c.ensure(c.dim, on: c.mantle)])
            guard line >= 0 else { continue }
            let text = side == .left ? s.model.left.lines[line] : s.model.right.lines[line]
            let a = pane.attributed(text.replacingOccurrences(of: "\t", with: "→"), color: c.text,
                                    marks: side == .left ? mk.left : mk.right, c: c, bg: c.mantle)
            NSGraphicsContext.saveGraphicsState()
            NSRect(x: 80, y: y, width: bounds.width - 90, height: pane.rowH).clip()
            a.draw(at: NSPoint(x: 80, y: y))
            NSGraphicsContext.restoreGraphicsState()
        }
    }
}

final class CompareEditor: NSTextView {
    var side: CompareSide = .left
    var lines: Range<Int> = 0..<0
    var rows: Range<Int> = 0..<0
    var original = ""
    var minHeight: CGFloat = 18
    var borderColor = NSColor.controlAccentColor
    var onResize: (() -> Void)?

    override func draw(_ dirty: NSRect) {
        super.draw(dirty)
        borderColor.setStroke()
        let p = NSBezierPath(rect: bounds.insetBy(dx: 0.75, dy: 0.75))
        p.lineWidth = 1.5
        p.stroke()
    }

    override func didChangeText() {
        super.didChangeText()
        fit()
    }

    func fit() {
        guard let lm = layoutManager, let tc = textContainer else { return }
        lm.ensureLayout(for: tc)
        let h = max(minHeight, ceil(lm.usedRect(for: tc).height) + textContainerInset.height * 2 + 2)
        if abs(frame.height - h) > 0.5 {
            setFrameSize(NSSize(width: frame.width, height: h))
            onResize?()
        }
    }

    var editedLines: [String] {
        var t = string.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        if t.isEmpty { return [] }
        if t.hasSuffix("\n") { t.removeLast() }
        return t.components(separatedBy: "\n")
    }
}
