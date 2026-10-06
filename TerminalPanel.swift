import AppKit
import SwiftTerm

// The dedicated terminal (Ctrl+B T, palette /terminal, `kitchen-sink
// term`): the old notes drawer's shell as its own popup. A tool panel —
// borderless, non-activating, floating, ignored by AeroSpace — so it opens
// on the screen you're on and never brings the shared window along. The
// shell session survives hide / show (and restarts itself when it exits).
// Keys: everything goes to the shell (Esc, Ctrl+C …) except Cmd+C / V / A
// (copy / paste / select all), Cmd+K (clear), Cmd+= / Cmd+- (font), Cmd+W
// (hide). Drag the header to move, the corner grip to resize; the frame is
// remembered.

private final class TerminalNSPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

// the header strip: drags the window, ✕ hides it
private final class TerminalHeader: NSView {
    var colors: PopupColors
    var title = "Terminal" { didSet { needsDisplay = true } }
    var onClose: (() -> Void)?
    private var closeHover = false
    init(colors: PopupColors) {
        self.colors = colors
        super.init(frame: .zero)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    private var closeRect: NSRect { NSRect(x: 10, y: (bounds.height - 18) / 2, width: 18, height: 18) }
    override func draw(_ dirtyRect: NSRect) {
        let c = colors
        if closeHover {
            c.highlight.setFill()
            NSBezierPath(roundedRect: closeRect, xRadius: 5, yRadius: 5).fill()
        }
        let x: NSString = "✕"
        let xa: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11, weight: .medium),
                                                 .foregroundColor: closeHover ? c.text : c.dim]
        let xs = x.size(withAttributes: xa)
        x.draw(at: NSPoint(x: closeRect.midX - xs.width / 2, y: closeRect.midY - xs.height / 2), withAttributes: xa)
        let t = title as NSString
        let para = NSMutableParagraphStyle()
        para.alignment = .center
        para.lineBreakMode = .byTruncatingMiddle
        let ta: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11.5, weight: .medium),
                                                 .foregroundColor: c.dim, .paragraphStyle: para]
        let th = t.size(withAttributes: ta).height
        t.draw(in: NSRect(x: 40, y: (bounds.height - th) / 2, width: max(0, bounds.width - 80), height: th), withAttributes: ta)
        c.text.withAlphaComponent(0.08).setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }
    override func mouseMoved(with e: NSEvent) {
        let h = closeRect.insetBy(dx: -3, dy: -3).contains(convert(e.locationInWindow, from: nil))
        if h != closeHover { closeHover = h; needsDisplay = true }
    }
    override func mouseExited(with e: NSEvent) { if closeHover { closeHover = false; needsDisplay = true } }
    override func mouseDown(with e: NSEvent) {
        if closeRect.insetBy(dx: -3, dy: -3).contains(convert(e.locationInWindow, from: nil)) { onClose?(); return }
        window?.performDrag(with: e)
    }
}

// bottom-right corner: drag to resize (a borderless panel has no edges)
private final class TerminalGrip: NSView {
    var colors: PopupColors
    var onResized: (() -> Void)?
    private var start: (mouse: NSPoint, frame: NSRect)?
    init(colors: PopupColors) { self.colors = colors; super.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ dirtyRect: NSRect) {
        colors.dim.withAlphaComponent(0.6).setStroke()
        for i in 0..<3 {
            let p = NSBezierPath()
            let o = CGFloat(i) * 4 + 3
            p.move(to: NSPoint(x: bounds.maxX - o, y: 2))
            p.line(to: NSPoint(x: bounds.maxX - 2, y: o))
            p.lineWidth = 1
            p.stroke()
        }
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .crosshair) }
    override func mouseDown(with e: NSEvent) {
        guard let w = window else { return }
        start = (NSEvent.mouseLocation, w.frame)
    }
    override func mouseDragged(with e: NSEvent) {
        guard let w = window, let s = start else { return }
        let m = NSEvent.mouseLocation
        let dw = m.x - s.mouse.x, dh = s.mouse.y - m.y
        let width = max(360, s.frame.width + dw), height = max(200, s.frame.height + dh)
        w.setFrame(NSRect(x: s.frame.minX, y: s.frame.maxY - height, width: width, height: height), display: true)
    }
    override func mouseUp(with e: NSEvent) { start = nil; onResized?() }
}

final class TerminalPanel: NSObject, NSWindowDelegate {
    private let panel: TerminalNSPanel
    private let term: LocalProcessTerminalView
    private let header: TerminalHeader
    private let grip: TerminalGrip
    private var colors: PopupColors
    private var monitor: Any?
    private var restarter: TerminalAutoRestart?
    private var fontSize: CGFloat
    private let fontName: String
    private static let frameKey = "terminalPanelFrame"
    var log: ((String) -> Void)?
    private let headerH: CGFloat = 30

    var isShown: Bool { panel.isVisible }
    var isKey: Bool { panel.isKeyWindow }
    var windowNumber: Int { panel.windowNumber }

    init(colors: PopupColors, shell: String, args: [String],
         fontName: String, fontSize: CGFloat) {
        self.colors = colors
        self.fontName = fontName
        self.fontSize = fontSize
        panel = TerminalNSPanel(contentRect: NSRect(x: 0, y: 0, width: 760, height: 460),
                                styleMask: [.borderless, .nonactivatingPanel, .resizable],
                                backing: .buffered, defer: false)
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .none
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.title = "terminal"
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let root = NSView(frame: panel.contentRect(forFrameRect: panel.frame))
        root.wantsLayer = true
        root.layer?.cornerRadius = 12
        root.layer?.masksToBounds = true
        root.layer?.backgroundColor = colors.background.withAlphaComponent(1).cgColor
        root.layer?.borderWidth = 1
        root.layer?.borderColor = colors.text.withAlphaComponent(0.1).cgColor
        root.autoresizingMask = [.width, .height]
        panel.contentView = root
        header = TerminalHeader(colors: colors)
        grip = TerminalGrip(colors: colors)
        term = LocalProcessTerminalView(frame: .zero)
        super.init()
        panel.delegate = self
        root.addSubview(term)
        root.addSubview(header)
        root.addSubview(grip)
        term.font = NSFont(name: fontName, size: fontSize) ?? .monospacedSystemFont(ofSize: fontSize, weight: .regular)
        term.nativeBackgroundColor = colors.background.withAlphaComponent(1)
        term.nativeForegroundColor = colors.text
        term.installColors(PopupWindow.ansiPalette(colors))
        term.menu = menu()
        header.onClose = { [weak self] in self?.hide() }
        grip.onResized = { [weak self] in self?.saveFrame() }
        let r = TerminalAutoRestart()
        r.onTerminated = { [weak self] in
            DispatchQueue.main.async {
                self?.term.startProcess(executable: shell, args: args, currentDirectory: NSHomeDirectory())
            }
        }
        term.processDelegate = r
        restarter = r
        layout()
        term.startProcess(executable: shell, args: args, currentDirectory: NSHomeDirectory())
    }

    private func layout() {
        guard let root = panel.contentView else { return }
        let b = root.bounds
        header.frame = NSRect(x: 0, y: b.height - headerH, width: b.width, height: headerH)
        term.frame = NSRect(x: 8, y: 8, width: max(0, b.width - 16), height: max(0, b.height - headerH - 12))
        grip.frame = NSRect(x: b.width - 16, y: 0, width: 16, height: 16)
    }
    func windowDidResize(_ notification: Notification) { layout() }
    func windowDidMove(_ notification: Notification) { saveFrame() }

    private func saveFrame() {
        UserDefaults.standard.set(NSStringFromRect(panel.frame), forKey: Self.frameKey)
    }

    // ON the screen the mouse is on (= where you are): the remembered size,
    // centered there unless the remembered frame already is on that screen
    private func place() {
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main
        guard let vf = screen?.visibleFrame else { return }
        var f = NSRect(x: 0, y: 0, width: 760, height: 460)
        if let s = UserDefaults.standard.string(forKey: Self.frameKey) {
            let saved = NSRectFromString(s)
            if saved.width >= 360, saved.height >= 200 {
                if vf.intersects(saved), vf.contains(NSPoint(x: saved.midX, y: saved.midY)) {
                    panel.setFrame(saved, display: false)
                    return
                }
                f.size = NSSize(width: min(saved.width, vf.width), height: min(saved.height, vf.height))
            }
        }
        f.origin = NSPoint(x: vf.midX - f.width / 2, y: vf.midY - f.height / 2 + vf.height * 0.08)
        panel.setFrame(f, display: false)
    }

    // hidden → show + keyboard; shown but elsewhere → keyboard; key → hide
    func toggle() {
        if panel.isVisible && panel.isKeyWindow { hide(); return }
        show()
    }

    func show() {
        if !panel.isVisible { place() }
        layout()
        panel.orderFrontRegardless()
        panel.makeKey()                 // no activation: a tool panel
        panel.makeFirstResponder(term)
        installMonitor()
        log?("terminal panel shown")
    }

    func hide() {
        panel.orderOut(nil)
        if let m = monitor { NSEvent.removeMonitor(m); monitor = nil }
        log?("terminal panel hidden")
    }

    private func setFont(_ size: CGFloat) {
        fontSize = min(36, max(8, size))
        term.font = NSFont(name: fontName, size: fontSize) ?? .monospacedSystemFont(ofSize: fontSize, weight: .regular)
        layout()
    }

    private func installMonitor() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.panel.isKeyWindow else { return e }
            let m = e.modifierFlags.intersection([.command, .control, .option, .shift])
            guard m.contains(.command), !m.contains(.control) else { return e }
            switch e.charactersIgnoringModifiers?.lowercased() ?? "" {
            case "c": if self.term.selectedRange().length > 0 { self.term.copy(NSNull()) }; return nil
            case "v": self.term.paste(NSNull()); return nil
            case "a": self.term.selectAll(nil); return nil
            case "k": self.clear(); return nil
            case "w": self.hide(); return nil
            case "=", "+": self.setFont(self.fontSize + 1); return nil
            case "-": self.setFont(self.fontSize - 1); return nil
            default: return nil       // never Cmd+Q the daemon from here
            }
        }
    }

    private func clear() {
        term.send(txt: "\u{0C}")      // Ctrl+L: the shell clears + redraws its prompt
    }

    private func menu() -> NSMenu {
        let m = NSMenu(title: "Terminal")
        m.addItem(menuItem("Copy") { [weak self] in self?.term.copy(NSNull()) })
        m.addItem(menuItem("Paste") { [weak self] in self?.term.paste(NSNull()) })
        m.addItem(menuItem("Select All") { [weak self] in self?.term.selectAll(nil) })
        m.addItem(.separator())
        m.addItem(menuItem("Clear") { [weak self] in self?.clear() })
        m.addItem(menuItem("Bigger Font") { [weak self] in self.map { $0.setFont($0.fontSize + 1) } })
        m.addItem(menuItem("Smaller Font") { [weak self] in self.map { $0.setFont($0.fontSize - 1) } })
        m.addItem(.separator())
        m.addItem(menuItem("Hide Terminal") { [weak self] in self?.hide() })
        return m
    }

    // socket `state`
    func testState() -> [String: Any] {
        ["shown": panel.isVisible, "key": panel.isKeyWindow, "level": panel.level.rawValue,
         "wid": panel.windowNumber,
         "frame": [panel.frame.minX, panel.frame.minY, panel.frame.width, panel.frame.height].map { Int($0) },
         "running": term.process?.running ?? false]
    }
}
