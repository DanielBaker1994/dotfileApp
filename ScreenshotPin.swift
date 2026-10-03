import AppKit

// A pinned capture (the ring's Pin, `--pin`): a borderless, non-activating,
// always-on-top panel holding the image at 1:1 (AeroSpace ignores NSPanels,
// so it never tiles). Drag moves, wheel / pinch zooms 3% a step, 0…9 =
// opacity, double-click / Esc / Cmd+W / Cmd+Q close, Cmd+C copies, and a
// right-click menu. Keys arrive through ScreenshotController's monitor.
final class PinPanel: NSPanel {
    private(set) var image: CGImage
    var onClose: ((PinPanel) -> Void)?
    var onCopy: ((CGImage) -> Void)?
    var onSave: ((CGImage) -> Void)?
    private let pinView: PinView
    private var zoom: CGFloat = 1
    private var base: CGSize             // the image in points at 1:1
    static let margin: CGFloat = 6       // room for the soft shadow

    init(image: CGImage, frame f: CGRect, ui: ShotColor, contrast: ShotColor) {
        self.image = image
        base = f.size
        pinView = PinView(ui: ui, contrast: contrast)
        let m = Self.margin
        super.init(contentRect: f.insetBy(dx: -m, dy: -m), styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        level = .floating
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        isMovableByWindowBackground = false
        animationBehavior = .none
        collectionBehavior = [.fullScreenAuxiliary, .ignoresCycle]
        pinView.image = image
        pinView.panel = self
        contentView = pinView
    }
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func cancelOperation(_ sender: Any?) { closePin() }

    func present() {
        orderFrontRegardless()
        makeKey()
    }

    func closePin() {
        orderOut(nil)
        onClose?(self)
    }

    // MARK: zoom / rotate / opacity

    func setZoom(_ z: CGFloat, around p: CGPoint? = nil) {
        let b = base
        // min 100 px on the short side
        let minZ = 100 / max(1, min(b.width, b.height))
        zoom = max(min(1, minZ), min(8, z))
        let size = CGSize(width: (b.width * zoom).rounded() + Self.margin * 2, height: (b.height * zoom).rounded() + Self.margin * 2)
        let c = p ?? CGPoint(x: frame.midX, y: frame.midY)
        setFrame(CGRect(x: (c.x - size.width / 2).rounded(), y: (c.y - size.height / 2).rounded(),
                        width: size.width, height: size.height), display: true)
    }

    func rotate(_ quarters: Int) {
        pinView.quarterTurns = (quarters % 4 + 4) % 4
        image = pinView.renderedImage() ?? image
        pinView.quarterTurns = 0
        pinView.image = image
        if quarters % 2 != 0 { base = CGSize(width: base.height, height: base.width) }
        setZoom(zoom)
    }

    func opacity(_ delta: CGFloat) { alphaValue = max(0.1, min(1, alphaValue + delta)) }

    // MARK: input

    func handleKey(_ e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command)
        let ch = (e.charactersIgnoringModifiers ?? "").lowercased()
        if e.keyCode == 53 || (cmd && (ch == "w" || ch == "q")) { closePin(); return true }
        if cmd && ch == "c" { onCopy?(image); return true }
        if !cmd, let d = Int(ch), (0...9).contains(d) {
            alphaValue = d == 0 ? 1 : CGFloat(10 - d) / 10   // 0 = 100%, 1 = 90% … 9 = 10%
            return true
        }
        return true   // nothing reaches the daemon's menu
    }

    func menu() -> NSMenu {
        let m = NSMenu()
        m.addItem(menuItem("Copy to clipboard") { [weak self] in if let s = self { s.onCopy?(s.image) } })
        m.addItem(menuItem("Save to file") { [weak self] in if let s = self { s.onSave?(s.image) } })
        m.addItem(.separator())
        m.addItem(menuItem("Rotate Right") { [weak self] in self?.rotate(1) })
        m.addItem(menuItem("Rotate Left") { [weak self] in self?.rotate(-1) })
        m.addItem(menuItem("Increase Opacity") { [weak self] in self?.opacity(0.1) })
        m.addItem(menuItem("Decrease Opacity") { [weak self] in self?.opacity(-0.1) })
        m.addItem(.separator())
        m.addItem(menuItem("Close") { [weak self] in self?.closePin() })
        return m
    }

    fileprivate func wheel(_ e: NSEvent) {
        let dy = e.hasPreciseScrollingDeltas ? e.scrollingDeltaY / 10 : e.scrollingDeltaY
        guard dy != 0 else { return }
        setZoom(zoom * (dy > 0 ? 1.03 : 1 / 1.03))
    }
    fileprivate func pinch(_ e: NSEvent) { setZoom(zoom * (1 + e.magnification)) }
}

final class PinView: NSView {
    weak var panel: PinPanel?
    var image: CGImage? { didSet { needsDisplay = true } }
    var quarterTurns = 0 { didSet { needsDisplay = true } }
    private let ui: ShotColor, contrast: ShotColor
    private var hover = false { didSet { updateShadow() } }
    private var tracking: NSTrackingArea?

    init(ui: ShotColor, contrast: ShotColor) {
        self.ui = ui
        self.contrast = contrast
        super.init(frame: .zero)
        wantsLayer = true
        updateShadow()
    }
    required init?(coder: NSCoder) { fatalError() }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    private func updateShadow() {
        layer?.shadowColor = (hover ? contrast : ui).cgColor
        layer?.shadowOpacity = 1
        layer?.shadowRadius = 2
        layer?.shadowOffset = .zero
    }
    override func layout() {
        super.layout()
        layer?.shadowPath = CGPath(rect: bounds.insetBy(dx: PinPanel.margin, dy: PinPanel.margin), transform: nil)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true }
    override func mouseExited(with event: NSEvent) { hover = false }

    override func mouseDown(with e: NSEvent) {
        if e.clickCount == 2 { panel?.closePin(); return }
        panel?.makeKey()
        window?.performDrag(with: e)
    }
    override func scrollWheel(with e: NSEvent) { panel?.wheel(e) }
    override func magnify(with e: NSEvent) { panel?.pinch(e) }
    override func rightMouseDown(with e: NSEvent) {
        guard let m = panel?.menu() else { return }
        NSMenu.popUpContextMenu(m, with: e, for: self)
    }
    override func keyDown(with event: NSEvent) {}

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext, let image else { return }
        let r = bounds.insetBy(dx: PinPanel.margin, dy: PinPanel.margin)
        ctx.interpolationQuality = .high
        ctx.draw(image, in: r)
    }

    // the image turned by quarterTurns (90° clockwise each)
    func renderedImage() -> CGImage? {
        guard let image else { return nil }
        let q = ((quarterTurns % 4) + 4) % 4
        guard q != 0 else { return image }
        let w = q % 2 == 0 ? image.width : image.height
        let h = q % 2 == 0 ? image.height : image.width
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.translateBy(x: CGFloat(w) / 2, y: CGFloat(h) / 2)
        ctx.rotate(by: -CGFloat(q) * .pi / 2)
        ctx.draw(image, in: CGRect(x: -CGFloat(image.width) / 2, y: -CGFloat(image.height) / 2,
                                   width: CGFloat(image.width), height: CGFloat(image.height)))
        return ctx.makeImage()
    }
}
