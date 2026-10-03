import AppKit

// Quick, read-only look at a file: double-click / Enter on a picture or a text
// file in the file browser (and the Hyper+S recent list), or click a picture
// in a note, opens it in its own borderless panel. Pinch (or Cmd +/-/0, or a
// two-finger double-tap) zooms; dragging a corner resizes it proportionally, an edge freely, dragging the body (text: its top strip) moves it; Esc closes and hands the keyboard back;
// clicking elsewhere closes it too. A non-activating panel — the app is never
// re-activated, so the shared window and AeroSpace are untouched (like the
// tool panels). Right-click ▸ Open With in the browsers still picks any app.
final class FilePopupPanel: NSPanel {
    var onEscape: (() -> Void)?
    weak var zoomView: NSScrollView?
    weak var textView: NSTextView?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func cancelOperation(_ sender: Any?) { onEscape?() }
    // Mouse-down in the border band = resize (tracked here, not through the
    // subview hit-test, so the scroll view can never swallow it); elsewhere a
    // picture drags the whole panel, text only by its top strip (the rest
    // selects text).
    override func sendEvent(_ event: NSEvent) {
        if event.type == .leftMouseDown, event.window === self, let box = contentView {
            let p = event.locationInWindow, w = box.bounds.width, h = box.bounds.height
            let g = FilePopupGrip.size, e = FilePopupGrip.edge
            let corner = (p.x < g || p.x > w - g) && (p.y < g || p.y > h - g)
            let lim = corner ? g : e
            let dx = p.x < lim ? -1 : (p.x > w - lim ? 1 : 0)
            let dy = p.y < lim ? -1 : (p.y > h - lim ? 1 : 0)
            if dx != 0 || dy != 0,
               let grip = box.subviews.compactMap({ $0 as? FilePopupGrip }).first(where: { $0.dx == dx && $0.dy == dy }) {
                grip.mouseDown(with: event)
                while let n = nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
                    if n.type == .leftMouseUp { grip.mouseUp(with: n); break }
                    grip.mouseDragged(with: n)
                }
                return
            }
            if textView == nil || p.y > h - 26 {
                performDrag(with: event)
                return
            }
        }
        super.sendEvent(event)
    }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onEscape?() } else { super.keyDown(with: event) }
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
              let sv = zoomView else { return super.performKeyEquivalent(with: event) }
        let c = NSPoint(x: sv.contentView.bounds.midX, y: sv.contentView.bounds.midY)
        switch event.charactersIgnoringModifiers ?? "" {
        case "=", "+": sv.setMagnification(min(sv.maxMagnification, sv.magnification * 1.25), centeredAt: c)
        case "-": sv.setMagnification(max(sv.minMagnification, sv.magnification / 1.25), centeredAt: c)
        case "0": sv.setMagnification(1, centeredAt: c)
        case "w": onEscape?()
        case "c": textView?.copy(nil)
        case "a": textView?.selectAll(nil)
        default: return super.performKeyEquivalent(with: event)
        }
        return true
    }
}

// Invisible resize handle on a panel edge or corner. A corner keeps the
// proportions (the opposite corner stays put, the content zooms by the same
// factor); an edge resizes that side freely (an image is scaled to cover the
// new frame and scrolls on the other axis, text just shows more or less).
final class FilePopupGrip: NSView {
    // dx / dy: -1 = the left / bottom side moves, 1 = right / top, 0 = not
    let dx: Int, dy: Int
    weak var scroll: NSScrollView?
    private var start: (frame: NSRect, mouse: NSPoint, mag: CGFloat, anchor: NSPoint)?
    static let size: CGFloat = 22
    static let edge: CGFloat = 8

    init(dx: Int, dy: Int, scroll: NSScrollView) {
        self.dx = dx
        self.dy = dy
        self.scroll = scroll
        super.init(frame: .zero)
        var m: NSView.AutoresizingMask = []
        if dx == 0 { m.insert(.width) } else { m.insert(dx < 0 ? .maxXMargin : .minXMargin) }
        if dy == 0 { m.insert(.height) } else { m.insert(dy < 0 ? .maxYMargin : .minYMargin) }
        autoresizingMask = m
    }
    required init?(coder: NSCoder) { fatalError() }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func resetCursorRects() {
        var c = NSCursor.arrow
        if dx == 0 { c = .resizeUpDown } else if dy == 0 { c = .resizeLeftRight } else {
            // AppKit's private diagonal resize cursors; arrow if they go away
            let sel = Selector(dx * dy > 0 ? "_windowResizeNorthEastSouthWestCursor"
                                           : "_windowResizeNorthWestSouthEastCursor")
            if NSCursor.responds(to: sel), let d = NSCursor.perform(sel)?.takeUnretainedValue() as? NSCursor { c = d }
        }
        addCursorRect(bounds, cursor: c)
    }

    override func mouseDown(with event: NSEvent) {
        guard let w = window else { return }
        // anchor = the visible rect's left and top as fractions of the picture
        var anchor = NSPoint.zero
        if let sv = scroll, let doc = sv.documentView, doc.bounds.width > 0, doc.bounds.height > 0 {
            let vis = sv.documentVisibleRect
            anchor = NSPoint(x: vis.minX / doc.bounds.width,
                             y: max(0, doc.bounds.height - vis.maxY) / doc.bounds.height)
        }
        start = (w.frame, NSEvent.mouseLocation, scroll?.magnification ?? 1, anchor)
    }

    override func mouseDragged(with event: NSEvent) {
        guard let w = window, let st = start, st.frame.width > 0, st.frame.height > 0 else { return }
        let m = NSEvent.mouseLocation
        let dw = CGFloat(dx) * (m.x - st.mouse.x), dh = CGFloat(dy) * (m.y - st.mouse.y)
        let visible = (w.screen ?? NSScreen.main)?.visibleFrame ?? st.frame
        var size = st.frame.size
        var k: CGFloat = 1
        if dx != 0 && dy != 0 {
            // follow whichever axis the pointer moved further, as a fraction
            k = max((st.frame.width + dw) / st.frame.width, (st.frame.height + dh) / st.frame.height)
            k = min(k, visible.width / st.frame.width, visible.height / st.frame.height)
            k = max(k, 120 / st.frame.width, 80 / st.frame.height)
            if let sv = scroll {
                k = min(max(k, sv.minMagnification / st.mag), sv.maxMagnification / st.mag)
            }
            size = NSSize(width: st.frame.width * k, height: st.frame.height * k)
        } else {
            if dx != 0 { size.width = min(max(st.frame.width + dw, 120), visible.width) }
            if dy != 0 { size.height = min(max(st.frame.height + dh, 80), visible.height) }
        }
        // the side opposite the dragged one stays put
        let x = dx < 0 ? st.frame.maxX - size.width : st.frame.minX
        let y = dy < 0 ? st.frame.maxY - size.height : st.frame.minY
        w.setFrame(NSRect(origin: NSPoint(x: x, y: y), size: size), display: true)
        guard let sv = scroll else { return }
        if dx != 0 && dy != 0 {
            sv.magnification = st.mag * k
        } else if let iv = sv.documentView as? NSImageView, let img = iv.image,
                  img.size.width > 0, img.size.height > 0 {
            // free resize: the picture COVERS the panel (proportions kept, never
            // letterboxed) — widening a long scrollback shot makes it wider and
            // the rest scrolls; fitting inside would pin it to the height
            let s = max(size.width / img.size.width, size.height / img.size.height)
            let doc = NSSize(width: max(size.width, (img.size.width * s).rounded()),
                             height: max(size.height, (img.size.height * s).rounded()))
            sv.magnification = 1
            iv.frame = NSRect(origin: .zero, size: doc)
            // the visible top-left keeps its place in the picture (unflipped doc)
            let x = min(max(0, st.anchor.x * doc.width), doc.width - size.width)
            let top = st.anchor.y * doc.height
            let y = min(max(0, doc.height - top - size.height), doc.height - size.height)
            sv.contentView.scroll(to: NSPoint(x: x, y: y))
            sv.reflectScrolledClipView(sv.contentView)
        }
    }

    override func mouseUp(with event: NSEvent) { start = nil }
}

// the popup's border ring; clicks fall through to the content / grips
final class FilePopupRing: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

enum FilePopup {
    private static var panel: FilePopupPanel?
    private static var resignObserver: NSObjectProtocol?
    private static let textLimit = 512 * 1024
    // [app] preview-border / preview-border-width: a silver ring so the
    // popup stands out (above all on the screen a capture was just taken of)
    static let defaultBorder = NSColor(srgbRed: 0.867, green: 0.882, blue: 0.91, alpha: 1)
    static var borderColor = defaultBorder
    static var borderWidth: CGFloat = 2

    static let imageExts: Set<String> = ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "bmp"]
    static func isImage(_ path: String) -> Bool {
        imageExts.contains((path as NSString).pathExtension.lowercased())
    }

    // the text of a text-like file (first `textLimit` bytes), nil for
    // anything binary: no NUL byte and valid UTF-8 in the sample
    static func previewText(_ path: String) -> String? {
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDir), !isDir.boolValue,
              let fh = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? fh.close() }
        let data = ((try? fh.read(upToCount: textLimit + 4)) ?? nil) ?? Data()
        if data.contains(0) { return nil }
        let cut = data.count > textLimit
        var body = cut ? data.prefix(textLimit) : data
        // a multi-byte character cut in half is not "invalid"
        var s = String(data: body, encoding: .utf8)
        var trim = 0
        while s == nil, trim < 3, !body.isEmpty { body = body.dropLast(); trim += 1; s = String(data: body, encoding: .utf8) }
        guard let text = s else { return nil }
        return cut ? text + "\n\n… (preview stops at \(textLimit / 1024) KB)" : text
    }

    // the default "open" for files: pictures and text pop up as a preview,
    // the rest go to their default app
    static func open(_ path: String, over parent: NSWindow?) {
        if isImage(path), NSImage(contentsOfFile: path) != nil {
            show(path: path, over: parent)
        } else if !isImage(path), let text = previewText(path) {
            showText(text, path: path, over: parent)
        } else {
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
    }

    private static func zoomScroll(_ doc: NSView, _ frame: NSRect) -> NSScrollView {
        let sv = NSScrollView(frame: frame)
        sv.documentView = doc
        sv.allowsMagnification = true
        sv.minMagnification = 0.25
        sv.maxMagnification = 8
        sv.hasVerticalScroller = true
        sv.hasHorizontalScroller = true
        sv.autohidesScrollers = true
        sv.scrollerStyle = .overlay
        sv.drawsBackground = false
        sv.borderType = .noBorder
        sv.wantsLayer = true
        sv.layer?.cornerRadius = 8
        sv.layer?.masksToBounds = true
        return sv
    }

    private static func present(_ scroll: NSScrollView, frame: NSRect, parent: NSWindow?,
                                label: String, text: NSTextView? = nil) {
        close()
        let p = FilePopupPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                               backing: .buffered, defer: false)
        p.isReleasedWhenClosed = false
        p.isFloatingPanel = true
        p.level = .floating
        p.hidesOnDeactivate = false
        p.hasShadow = true
        p.isOpaque = false
        p.backgroundColor = .clear
        p.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        let box = NSView(frame: NSRect(origin: .zero, size: frame.size))
        scroll.frame = box.bounds
        scroll.autoresizingMask = [.width, .height]
        box.addSubview(scroll)
        // the ring: its own click-through view over the content (a border
        // on the scroll view's own layer never shows — AppKit owns it)
        if borderWidth > 0 {
            let ring = FilePopupRing(frame: box.bounds)
            ring.autoresizingMask = [.width, .height]
            ring.wantsLayer = true
            ring.layer?.cornerRadius = 8
            ring.layer?.borderWidth = borderWidth
            ring.layer?.borderColor = borderColor.cgColor
            box.addSubview(ring)
        }
        let g = FilePopupGrip.size, e = FilePopupGrip.edge
        let w = frame.width, h = frame.height
        // edges first, so the corners sit on top of them
        let handles: [(Int, Int, NSRect)] = [
            (-1, 0, NSRect(x: 0, y: g, width: e, height: h - 2 * g)),
            (1, 0, NSRect(x: w - e, y: g, width: e, height: h - 2 * g)),
            (0, -1, NSRect(x: g, y: 0, width: w - 2 * g, height: e)),
            (0, 1, NSRect(x: g, y: h - e, width: w - 2 * g, height: e)),
            (-1, -1, NSRect(x: 0, y: 0, width: g, height: g)),
            (1, -1, NSRect(x: w - g, y: 0, width: g, height: g)),
            (-1, 1, NSRect(x: 0, y: h - g, width: g, height: g)),
            (1, 1, NSRect(x: w - g, y: h - g, width: g, height: g)),
        ]
        for (dx, dy, r) in handles {
            let grip = FilePopupGrip(dx: dx, dy: dy, scroll: scroll)
            grip.frame = r
            box.addSubview(grip)
        }
        p.contentView = box
        p.zoomView = scroll
        p.textView = text
        p.setAccessibilityLabel(label)
        p.onEscape = { [weak parent] in
            close()
            parent?.makeKey()
        }
        // key lost = the user clicked elsewhere: just go away
        resignObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didResignKeyNotification, object: p, queue: .main
        ) { _ in close() }
        panel = p
        // front + key without activating: works while another app is
        // frontmost (a capture fired from a terminal)
        p.orderFrontRegardless()
        p.makeKey()
        if let text { p.makeFirstResponder(text) }
    }

    private static func room(for parent: NSWindow?, on screen: NSScreen? = nil) -> NSRect {
        (screen ?? parent?.screen ?? NSScreen.main ?? NSScreen.screens[0]).visibleFrame.insetBy(dx: 24, dy: 24)
    }

    static func show(path: String, over parent: NSWindow?) {
        guard let img = NSImage(contentsOfFile: path) else { return }
        show(image: img, label: (path as NSString).lastPathComponent, over: parent)
    }

    // a fresh capture (/screenshot, pane-shot): `img.size` in points
    static func show(image img: NSImage, label: String, over parent: NSWindow?, on screen: NSScreen? = nil) {
        guard img.size.width > 0, img.size.height > 0 else { return }
        let room = room(for: parent, on: screen)
        // natural size (points), shrunk to fit the screen, never upscaled
        let scale = min(1, room.width / img.size.width, room.height / img.size.height)
        let size = NSSize(width: floor(img.size.width * scale), height: floor(img.size.height * scale))
        let frame = NSRect(x: room.midX - size.width / 2, y: room.midY - size.height / 2,
                           width: size.width, height: size.height)
        let iv = NSImageView(frame: NSRect(origin: .zero, size: size))
        iv.image = img
        iv.imageScaling = .scaleProportionallyUpOrDown
        present(zoomScroll(iv, NSRect(origin: .zero, size: size)), frame: frame, parent: parent, label: label)
    }

    static func showText(_ text: String, path: String, over parent: NSWindow?) {
        let colors = (parent?.delegate as? PopupWindow)?.config.colors ?? PopupColors()
        let room = room(for: parent)
        let tv = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
        tv.isEditable = false
        tv.isSelectable = true
        tv.isRichText = false
        tv.drawsBackground = false
        tv.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        tv.textColor = colors.text
        tv.insertionPointColor = .clear
        tv.selectedTextAttributes = ButtonStyle.selection(colors)
        tv.textContainerInset = NSSize(width: 14, height: 12)
        // no wrapping: code stays readable; scroll / pinch to move around
        tv.isHorizontallyResizable = true
        tv.isVerticallyResizable = true
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.textContainer?.widthTracksTextView = false
        tv.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude,
                                                 height: CGFloat.greatestFiniteMagnitude)
        tv.string = text
        if let lm = tv.layoutManager, let tc = tv.textContainer {
            lm.ensureLayout(for: tc)
            let used = lm.usedRect(for: tc)
            let w = min(room.width, max(420, min(980, used.width + 40)))
            let h = min(room.height * 0.9, max(160, used.height + 30))
            tv.setFrameSize(NSSize(width: max(w, used.width + 30), height: used.height + 24))
            let frame = NSRect(x: room.midX - w / 2, y: room.midY - h / 2, width: w, height: h)
            let sv = zoomScroll(tv, NSRect(origin: .zero, size: frame.size))
            sv.drawsBackground = true
            sv.backgroundColor = colors.background.withAlphaComponent(1)
            present(sv, frame: frame, parent: parent,
                    label: (path as NSString).lastPathComponent, text: tv)
        }
    }

    static func close() {
        if let o = resignObserver { NotificationCenter.default.removeObserver(o) }
        resignObserver = nil
        let p = panel
        panel = nil
        p?.orderOut(nil)
    }
}
