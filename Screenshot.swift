import AppKit
import ScreenCaptureKit

// /screenshot (Hyper+X): a Flameshot-style capture + annotate tool. A TOOL
// PANEL (AGENT_CONTEXT "Tool panels"): it never activates the app, never
// touches the shared window, and AeroSpace never sees its windows.
//   ScreenshotAnnotations.swift  the model (tested: bin/run-tests.sh screenshot)
//   ScreenshotOverlay.swift      the capture screen (panels, session, views)
//   ScreenshotPin.swift          pinned captures
//   this file                    config, capture, outputs, CLI, test hooks

// MARK: - Config ([screenshot] in commands.toml)

struct ScreenshotConfig {
    var enabled = true
    var uiColor = ShotColor(hex: "#740096")!
    var contrastColor = ShotColor(hex: "#270032")!
    var contrastOpacity = 190
    var drawColor = ShotColor(hex: "#ff0000")!
    var userColors: [ShotColor?] = []
    var buttons: [ShotTool] = ShotTool.ring(ShotTool.defaultButtons, badge: true)
    var buttonSize: CGFloat = 34
    var showHelp = true
    var showSidePanelButton = true
    var magnifier = false
    var squareMagnifier = false
    var copyOnDoubleClick = false
    var returnAction = "copy"
    var savePath = "~/Desktop"
    var savePathFixed = false
    var filenamePattern = "%F_%H-%M"
    var saveFormat = "png"
    var jpegQuality = 75
    var saveAfterCopy = false
    var copyPathAfterSave = false
    var preview = true
    var saveLastRegion = false
    var undoLimit = 100
    var arrowStyle = 0
    var reverseArrow = false
    var counterOutline = true
    var insecurePixelate = false
    var delayMs = 0
    var font = ""
    var copyToast = "Capture saved to clipboard"
    var saveToast = "Capture saved as {}"
    var permissionToast = "Screen Recording permission needed — opening Settings…"
    var failToast = "Screen capture failed"
    var helpRows: [(String, String)] = []
    // Copy Text mode
    var startText = false
    var ocr = ShotOCRConfig()
    var textToast = "Copied {} to clipboard"
    var noTextToast = "No text found"
    var helpTextRows: [(String, String)] = []
    var shortcuts: [(String, String)] = []

    static let defaultUserColors = "picker, #800000, #ff0000, #ffff00, #00ff00, #008000, #00ffff, #0000ff, #ff00ff, #800080"
    static let helpKeys: [(key: String, label: String, text: String)] = [
        ("help-mouse", "Mouse", "Select screenshot area"),
        ("help-save", "⌘S", "Save screenshot to a file"),
        ("help-copy", "⌘C", "Copy selection to clipboard"),
        ("help-wheel", "Mouse Wheel", "Change tool size"),
        ("help-right-click", "Right Click", "Show color picker"),
        ("help-space", "Space", "Open side panel"),
        ("help-copy-text", "Tab", "Copy text mode"),
        ("help-esc", "Esc", "Exit"),
    ]
    static let helpTextKeys: [(key: String, label: String, text: String)] = [
        ("help-text", "Mouse", "Drag over text to copy it"),
        ("help-text-mode", "Tab", "Back to screenshot mode"),
        ("help-esc", "Esc", "Exit"),
    ]

    static func load() -> ScreenshotConfig {
        var c = ScreenshotConfig()
        let lines = readConfigText().map(configLines) ?? []
        var e: [String: String] = [:]
        for x in configSectionEntries(lines, "screenshot") { e[x.key] = x.value }
        func b(_ k: String, _ d: Bool) -> Bool { tri(e[k]) ?? d }
        func i(_ k: String, _ d: Int, _ r: ClosedRange<Int>) -> Int {
            guard let v = e[k].flatMap({ Double($0) }) else { return d }
            return max(r.lowerBound, min(r.upperBound, Int(v)))
        }
        func s(_ k: String, _ d: String) -> String {
            guard let v = e[k], !v.isEmpty else { return d }
            return v
        }
        func col(_ k: String, _ d: ShotColor) -> ShotColor {
            guard let v = e[k]?.trimmingCharacters(in: .whitespaces), !v.isEmpty else { return d }
            if v.lowercased() == "theme" { return ShotColor(ACCENT) }
            return ShotColor(hex: v) ?? d
        }
        c.enabled = b("enabled", true)
        c.uiColor = col("ui-color", c.uiColor)
        c.contrastColor = col("contrast-color", c.contrastColor)
        c.contrastOpacity = i("contrast-opacity", 190, 0...255)
        c.drawColor = col("draw-color", c.drawColor)
        c.userColors = s("user-colors", defaultUserColors).split(separator: ",").compactMap { part -> ShotColor?? in
            let v = part.trimmingCharacters(in: .whitespaces)
            if v.lowercased() == "picker" { return .some(nil) }
            return ShotColor(hex: v).map { .some($0) }
        }
        c.buttons = ShotTool.ring(s("buttons", ShotTool.defaultButtons), badge: b("show-size-badge", true))
        let bs = i("button-size", 0, 0...80)
        let font = NSFont.systemFont(ofSize: 13)
        c.buttonSize = bs >= 20 ? CGFloat(bs)
            : ButtonRing.defaultButtonSize(lineHeight: NSLayoutManager().defaultLineHeight(for: font))
        c.showHelp = b("show-help", true)
        c.showSidePanelButton = b("show-side-panel-button", true)
        c.magnifier = b("magnifier", false)
        c.squareMagnifier = b("square-magnifier", false)
        c.copyOnDoubleClick = b("copy-on-double-click", false)
        let r = s("return", "copy").lowercased()
        c.returnAction = ["copy", "save", "pin"].contains(r) ? r : "copy"
        c.savePath = s("save-path", "~/Desktop")
        c.savePathFixed = b("save-path-fixed", false)
        c.filenamePattern = s("filename-pattern", "%F_%H-%M")
        let f = s("save-format", "png").lowercased()
        c.saveFormat = ["jpg", "jpeg"].contains(f) ? "jpg" : "png"
        c.jpegQuality = i("jpeg-quality", 75, 1...100)
        c.saveAfterCopy = b("save-after-copy", false)
        c.copyPathAfterSave = b("copy-path-after-save", false)
        c.preview = b("preview", true)
        c.saveLastRegion = b("save-last-region", false)
        c.undoLimit = i("undo-limit", 100, 1...1000)
        c.arrowStyle = i("arrow-style", 0, 0...1)
        c.reverseArrow = b("reverse-arrow", false)
        c.counterOutline = b("counter-outline", true)
        c.insecurePixelate = b("insecure-pixelate", false)
        c.delayMs = i("delay", 0, 0...60_000)
        c.font = e["font"] ?? ""
        c.copyToast = e["copy-toast"] ?? c.copyToast
        c.saveToast = e["save-toast"] ?? c.saveToast
        c.permissionToast = s("permission-toast", c.permissionToast)
        c.failToast = s("fail-toast", c.failToast)
        c.helpRows = helpKeys.map { ($0.label, s($0.key, $0.text)) }
        c.helpTextRows = helpTextKeys.map { ($0.label, s($0.key, $0.text)) }
        c.startText = s("start-mode", "screenshot").lowercased() == "text"
        c.ocr.languages = s("text-languages", "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        c.ocr.correction = b("text-correction", true)
        c.textToast = e["text-toast"] ?? c.textToast
        c.noTextToast = s("no-text-toast", c.noTextToast)
        c.shortcuts = shortcutEntries.filter { $0.view == "screenshot" }.map { ($0.keys, $0.what) }
        return c
    }
}

extension ShotColor {
    init(_ ns: NSColor) {
        let c = ns.usingColorSpace(.sRGB) ?? ns
        self.init(r: Double(c.redComponent), g: Double(c.greenComponent), b: Double(c.blueComponent), a: Double(c.alphaComponent))
    }
}

extension NSScreen {
    var displayID: CGDirectDisplayID? {
        (deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber).map { CGDirectDisplayID($0.uint32Value) }
    }
}

// MARK: - Screen toast

// The Raycast pill on its own tiny non-activating panel, bottom-center of a
// screen (a shared-window toast would show that window).
enum ScreenToast {
    private static var panel: NSPanel?

    static func show(_ text: String, on screen: NSScreen?, symbol: String? = "checkmark.circle.fill") {
        guard !text.isEmpty, let scr = screen ?? NSScreen.main else { return }
        panel?.orderOut(nil)
        let pill = makeToastPill(text, symbol: symbol, colors: windowColors(), zoom: 1, maxWidth: scr.frame.width - 64)
        let pad: CGFloat = 18
        let w = pill.frame.width + pad * 2, h = pill.frame.height + pad * 2 + 6
        let p = NSPanel(contentRect: NSRect(x: (scr.frame.midX - w / 2).rounded(), y: scr.frame.minY + 70, width: w, height: h),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        p.level = NSWindow.Level(rawValue: NSWindow.Level.screenSaver.rawValue + 1)
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hasShadow = false
        p.ignoresMouseEvents = true
        p.hidesOnDeactivate = false
        p.isReleasedWhenClosed = false
        p.animationBehavior = .none
        p.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        let root = NSView(frame: NSRect(x: 0, y: 0, width: w, height: h))
        root.wantsLayer = true
        p.contentView = root
        pill.setFrameOrigin(CGPoint(x: pad, y: pad))
        root.addSubview(pill)
        p.orderFrontRegardless()
        panel = p
        animateToastPill(pill, rise: -6) {
            p.orderOut(nil)
            if panel === p { panel = nil }
        }
    }
}

// MARK: - Recent screenshots panel

final class ShotRecentPanel: NSPanel {
    private let recentView: ShotRecentView
    
    init(recent: [ShotRecentEntry], ui: NSColor, onSelect: @escaping (String) -> Void) {
        recentView = ShotRecentView(entries: recent, ui: ui, onSelect: onSelect)
        let w = CGFloat(min(360, max(200, recent.reduce(0) { max($0, $1.path.count) })))
        let h = CGFloat(min(recent.count, 10)) * 24 + 20
        let rect = NSRect(x: 0, y: 0, width: w, height: h)
        super.init(contentRect: rect, styleMask: .borderless, backing: .buffered, defer: false)
        contentView = recentView
        isFloatingPanel = true
        level = .screenSaver
        hidesOnDeactivate = false
        backgroundColor = .clear
        isOpaque = false
    }
}

struct ShotRecentEntry {
    var path: String
    var name: String
    var at: Date
}

final class ShotRecentView: NSView {
    private let entries: [ShotRecentEntry]
    private let ui: NSColor
    private let onSelect: (String) -> Void
    private let rowH: CGFloat = 24
    
    init(entries: [ShotRecentEntry], ui: NSColor, onSelect: @escaping (String) -> Void) {
        self.entries = entries
        self.ui = ui
        self.onSelect = onSelect
        super.init(frame: NSRect(x: 0, y: 0, width: 360, height: CGFloat(entries.count) * 24 + 20))
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    
    override func draw(_ dirty: NSRect) {
        let bg = NSColor(white: 0.12, alpha: 0.95)
        let r = NSBezierPath(roundedRect: bounds, xRadius: 8, yRadius: 8)
        bg.setFill(); r.fill()
        let font = NSFont.systemFont(ofSize: 11.5, weight: .medium)
        let dimFont = NSFont.systemFont(ofSize: 10.5)
        for (i, e) in entries.enumerated() {
            let row = NSRect(x: 4, y: CGFloat(i) * rowH + 4, width: bounds.width - 8, height: rowH)
            guard row.intersects(dirty) else { continue }
            (e.name as NSString).draw(in: NSRect(x: 8, y: row.minY + 3, width: bounds.width - 16, height: 16),
                                      withAttributes: [.font: font, .foregroundColor: NSColor.white])
            let age = ageString(e.at)
            let p = NSMutableParagraphStyle(); p.alignment = .right
            (age as NSString).draw(in: NSRect(x: 8, y: row.minY + 3, width: bounds.width - 16, height: 16),
                                   withAttributes: [.font: dimFont, .foregroundColor: NSColor(white: 0.5, alpha: 1), .paragraphStyle: p])
        }
    }
    override func mouseDown(with e: NSEvent) {
        let pt = convert(e.locationInWindow, from: nil)
        let i = Int((pt.y - 4) / rowH)
        guard entries.indices.contains(i) else { return }
        onSelect(entries[i].path)
    }
    
    private func ageString(_ d: Date) -> String {
        let s = Date().timeIntervalSince(d)
        if s < 60 { return "just now" }
        if s < 3600 { return "\(Int(s / 60))m ago" }
        if s < 86400 { return "\(Int(s / 3600))h ago" }
        let f = DateFormatter(); f.dateFormat = s < 7 * 86400 ? "EEE" : "MM-dd"
        return f.string(from: d)
    }
}

// MARK: - Controller

final class ScreenshotController {
    var log: (String) -> Void = { wsLog($0) }
    // a file the tool wrote (→ /paths) / the pasteboard write is ours (ClipboardPaths)
    var onSaved: ((String) -> Void)?
    var onOwnPasteboardWrite: (() -> Void)?

    private var panels: [CGDirectDisplayID: ShotOverlayPanel] = [:]
    private(set) var session: ShotSession?
    private var reply: ((Data?) -> Void)?
    private var content: SCShareableContent?
    private var keyMonitor: Any?
    private(set) var pins: [PinPanel] = []
    private var permissionAsked = false
    private var capturing = false
    private var forcedSavePath: String?
    private var lastOutput: [String: Any] = [:]
    // /pane-shot's last result (state `paneShot`)
    private(set) var paneShotLast: [String: Any] = [:]
    private var recentPanel: ShotRecentPanel?
    let statePath = NSHomeDirectory() + "/.cache/workspace-switcher/screenshot-state.json"

    init() {
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification,
                                               object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            self.content = nil
            if self.session == nil { self.prewarm() }
        }
    }

    // MARK: prewarm

    // one hidden overlay panel per display + the shareable content, so a
    // hotkey pays only for the capture itself
    func prewarm() {
        let ids = Set(NSScreen.screens.compactMap(\.displayID))
        for id in panels.keys where !ids.contains(id) { panels[id] = nil }
        for s in NSScreen.screens {
            guard let id = s.displayID else { continue }
            if let p = panels[id] { p.fit(s) } else { panels[id] = ShotOverlayPanel(screen: s) }
        }
        if content == nil, CGPreflightScreenCaptureAccess() { fetchContent { _ in } }
        warmOCR()
    }

    // Copy Text's model, compiled once per process in the background (the
    // first recognition otherwise stalls for ~50 s)
    private var ocrWarm = false
    private func warmOCR() {
        guard !ocrWarm else { return }
        ocrWarm = true
        let cfg = ScreenshotConfig.load().ocr
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let ms = ShotOCR.warmUp(cfg)
            DispatchQueue.main.async { self?.log("screenshot: text recognizer warm, \(ms) ms") }
        }
    }

    private func fetchContent(_ done: @escaping (SCShareableContent?) -> Void) {
        if let c = content { done(c); return }
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { [weak self] c, err in
            DispatchQueue.main.async {
                if let err { self?.log("screenshot: shareable content: \(err.localizedDescription)") }
                if let c { self?.content = c }
                done(c)
            }
        }
    }

    // MARK: permission

    static var permitted: Bool { CGPreflightScreenCaptureAccess() }

    // false (and a toast + the Privacy pane) when Screen Recording is off:
    // never an overlay of black / wallpaper-only pixels
    private func checkPermission(_ cfg: ScreenshotConfig) -> Bool {
        if Self.permitted { return true }
        if !permissionAsked {
            permissionAsked = true
            CGRequestScreenCaptureAccess()
        }
        ScreenToast.show(cfg.permissionToast, on: mouseScreen, symbol: "exclamationmark.triangle.fill")
        if let u = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") {
            NSWorkspace.shared.open(u)
        }
        log("screenshot: no Screen Recording permission")
        return false
    }

    var mouseScreen: NSScreen? {
        NSScreen.screens.first { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) } ?? NSScreen.main
    }

    // MARK: entry points

    // `screenshot [gui|full|screen] [flags]` from the socket / CLI / palette.
    // `reply` (when set) gets the result exactly once: PNG (-r), "W H X Y"
    // (-g), else empty; nil = aborted / failed.
    func handle(_ words: [String], reply: ((Data?) -> Void)? = nil) {
        switch ShotArgs.parse(words) {
        case .failure(let p):
            log("screenshot: \(p.message)")
            reply?(nil)
        case .success(let a):
            trigger(a, reply: reply)
        }
    }

    func trigger(_ args: ShotArgs, reply: ((Data?) -> Void)? = nil, extraDelay: Double = 0) {
        let cfg = ScreenshotConfig.load()
        guard cfg.enabled else {
            log("screenshot: [screenshot] enabled = false")
            reply?(nil)
            return
        }
        if session != nil || capturing {
            // a second hotkey while it's up: the keyboard back to the overlay
            if let d = session?.mouseDisplay { d.panel.orderFrontRegardless(); d.panel.makeKey() }
            reply?(nil)
            return
        }
        guard checkPermission(cfg) else { reply?(nil); return }
        capturing = true
        let t0 = DispatchTime.now().uptimeNanoseconds
        let delay = Double(args.delayMs > 0 ? args.delayMs : cfg.delayMs) / 1000 + extraDelay
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self else { return }
            let screens = NSScreen.screens
            let tc = DispatchTime.now().uptimeNanoseconds
            self.capture(screens) { images in
                self.capturing = false
                let capMs = Double(DispatchTime.now().uptimeNanoseconds - tc) / 1_000_000
                guard !images.isEmpty else {
                    ScreenToast.show(cfg.failToast, on: self.mouseScreen, symbol: "exclamationmark.triangle.fill")
                    self.log("screenshot: capture failed")
                    reply?(nil)
                    return
                }
                switch args.mode {
                case .gui, .text:
                    self.begin(cfg, args, screens, images, reply: reply)
                    let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000 - delay * 1000
                    self.log(String(format: "screenshot: capture %.0f ms, %.0f ms to overlay", capMs, ms))
                case .full, .screen:
                    self.direct(cfg, args, screens, images, reply: reply)
                    self.log(String(format: "screenshot %@: capture %.0f ms", args.mode.rawValue, capMs))
                }
            }
        }
    }

    // MARK: capture

    private func capture(_ screens: [NSScreen], _ done: @escaping ([CGDirectDisplayID: CGImage]) -> Void) {
        fetchContent { [weak self] content in
            guard let content else { done([:]); return }
            let group = DispatchGroup()
            let lock = NSLock()
            var out: [CGDirectDisplayID: CGImage] = [:]
            for s in screens {
                guard let id = s.displayID, let d = content.displays.first(where: { $0.displayID == id }) else { continue }
                let filter = SCContentFilter(display: d, excludingWindows: [])
                let c = SCStreamConfiguration()
                c.width = Int((s.frame.width * s.backingScaleFactor).rounded())
                c.height = Int((s.frame.height * s.backingScaleFactor).rounded())
                c.showsCursor = false
                c.captureResolution = .best
                group.enter()
                SCScreenshotManager.captureImage(contentFilter: filter, configuration: c) { img, err in
                    lock.lock()
                    if let img { out[id] = img }
                    lock.unlock()
                    if let err { DispatchQueue.main.async { self?.log("screenshot: display \(id): \(err.localizedDescription)") } }
                    group.leave()
                }
            }
            group.notify(queue: .main) {
                // a display list gone stale (a monitor came / went): refetch next time
                if out.count < screens.count { self?.content = nil }
                done(out)
            }
        }
    }

    // MARK: session

    private func begin(_ cfg: ScreenshotConfig, _ args: ShotArgs, _ screens: [NSScreen],
                       _ images: [CGDirectDisplayID: CGImage], reply: ((Data?) -> Void)?) {
        let s = ShotSession(cfg: cfg, args: args, statePath: statePath)
        s.log = log
        for scr in screens {
            guard let id = scr.displayID, let img = images[id] else { continue }
            let panel = panels[id] ?? ShotOverlayPanel(screen: scr)
            panels[id] = panel
            panel.fit(scr)
            let canvas = ShotCanvas(base: img, scale: CGFloat(img.width) / max(1, scr.frame.width))
            panel.setImage(img, scale: canvas.scale)
            s.displays.append(ShotDisplay(screen: scr, id: id, canvas: canvas, panel: panel))
        }
        session = s
        self.reply = reply
        s.onFinish = { [weak self, weak s] o in
            guard let self, let s else { return }
            self.finish(s, o)
        }
        s.onRecent = { [weak self, weak s] in
            guard let self, let s else { return }
            self.showRecentScreenshots(s)
        }
        for d in s.displays { d.view.attach(s, d) }
        for d in s.displays { d.panel.orderFrontRegardless() }
        let key = s.mouseDisplay ?? s.displays[0]
        key.panel.makeKey()
        key.panel.makeFirstResponder(key.view)
        installKeyMonitor()
        // a region given up front (--region / --last-region)
        if let r = initialRegion(args, s) { s.testSelect(r.rect, on: r.display) }
        s.redrawAll()
    }

    private func initialRegion(_ a: ShotArgs, _ s: ShotSession) -> (rect: CGRect, display: ShotDisplay)? {
        if let reg = a.region {
            if reg.hasPrefix("screen"), let n = Int(reg.dropFirst(6)), s.displays.indices.contains(n) {
                return (s.displays[n].bounds, s.displays[n])
            }
            if let g = ShotArgs.parseRegion(reg) {
                for d in s.displays {
                    let o = d.globalOrigin
                    let local = g.offsetBy(dx: -o.x, dy: -o.y)
                    if d.bounds.contains(CGPoint(x: local.minX, y: local.minY)) { return (local, d) }
                }
            }
        }
        if a.lastRegion, let l = s.state.lastRegion, let d = s.displays.first(where: { $0.id == l.display }) {
            return (CGRect(x: l.x, y: l.y, width: l.w, height: l.h), d)
        }
        return nil
    }

    private func installKeyMonitor() {
        guard keyMonitor == nil else { return }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { [weak self] e in
            guard let self else { return e }
            if let s = self.session, !s.finished, e.window is ShotOverlayPanel {
                return s.handleKey(e) ? nil : e
            }
            if let pin = e.window as? PinPanel {
                return pin.handleKey(e) ? nil : e
            }
            return e
        }
    }
    private func dropKeyMonitorIfIdle() {
        guard session == nil, pins.isEmpty, let m = keyMonitor else { return }
        NSEvent.removeMonitor(m)
        keyMonitor = nil
    }

    // close every overlay; the keyboard returns to the frontmost app by
    // itself (it was never taken from it: no activation)
    func close() {
        recentPanel?.close()
        recentPanel = nil
        guard let s = session else { return }
        for d in s.displays {
            d.panel.orderOut(nil)
            d.view.detach()
            d.panel.setImage(nil, scale: 1)
        }
        session = nil
        dropKeyMonitorIfIdle()
    }

    private func answer(_ d: Data?) {
        let r = reply
        reply = nil
        r?(d)
    }

    // MARK: recent screenshots

    private func showRecentScreenshots(_ s: ShotSession) {
        let entries = PathShelf.shared.entries().filter { $0.why == .screenshot }.prefix(10)
        guard !entries.isEmpty else { return }
        recentPanel?.close()
        let shotEntries = entries.map { e in
            ShotRecentEntry(path: e.path, name: (e.path as NSString).lastPathComponent, at: Date(timeIntervalSince1970: e.at))
        }
        let ui = NSColor(calibratedRed: s.cfg.uiColor.r, green: s.cfg.uiColor.g, blue: s.cfg.uiColor.b, alpha: s.cfg.uiColor.a)
        let panel = ShotRecentPanel(recent: shotEntries, ui: ui) { [weak self] path in
            guard let self else { return }
            self.recentPanel?.close()
            self.recentPanel = nil
            // Quick Look the file
            if FileManager.default.fileExists(atPath: path) {
                NSWorkspace.shared.selectFile(path, inFileViewerRootedAtPath: "")
            }
        }
        // Place near the mouse or center of main screen
        if let md = s.mouseDisplay ?? s.displays.first {
            panel.setFrameOrigin(CGPoint(x: md.screen.frame.midX - panel.frame.width / 2, y: md.screen.frame.midY - panel.frame.height / 2))
        }
        panel.orderFrontRegardless()
        panel.makeKey()
        recentPanel = panel
    }


    // MARK: outputs

    private func finish(_ s: ShotSession, _ o: ShotOutcome) {
        // Copy Text reads the screen's pixels, never the drawings over them
        let img = s.render(objects: o != .text)
        let screen = s.active?.screen
        let geom = s.globalSelection
        // the selection's AppKit (bottom-left) frame: where a pin opens
        var globalFrame: CGRect?
        if let d = s.active, let r = s.selection {
            globalFrame = CGRect(x: d.screen.frame.minX + r.minX, y: d.screen.frame.maxY - r.maxY, width: r.width, height: r.height)
        }
        if s.cfg.saveLastRegion, o != .abort, let d = s.active, let r = s.selection {
            s.state.lastRegion = .init(display: d.id, x: Double(r.minX), y: Double(r.minY), w: Double(r.width), h: Double(r.height))
            s.state.save(statePath)
        }
        let cfg = s.cfg, args = s.args
        let forced = forcedSavePath
        forcedSavePath = nil
        close()
        guard o != .abort, let img else {
            log("screenshot: aborted")
            lastOutput = ["outcome": "abort"]
            answer(nil)
            return
        }
        if o == .text { deliverText(img, cfg, args, screen); return }
        lastOutput = ["outcome": o.rawValue, "size": [img.width, img.height]]
        var action = o
        if o == .accept {
            if args.pin { action = .pin }
            else if args.path != nil || args.clipboard || args.raw || args.printGeometry { action = .accept }
            else { action = ShotOutcome(rawValue: cfg.returnAction) ?? .copy }
        }
        switch action {
        case .copy:
            copy(img, cfg, screen)
            if cfg.saveAfterCopy { save(img, cfg, screen, to: nil) }
        case .save:
            save(img, cfg, screen, to: forced ?? s.chosenSavePath ?? args.path)
        case .pin:
            pin(img, frame: globalFrame, cfg)
        case .accept:
            // explicit -p / -c (and -r / -g answer below)
            if let p = args.path { save(img, cfg, screen, to: p) }
            if args.clipboard { copy(img, cfg, screen) }
        case .abort, .text: break
        }
        // the capture in the quick-look popup too (copy / save untouched);
        // not for a pin (already on screen) or a scripted -r / -g
        if cfg.preview, action != .pin, !args.raw, !args.printGeometry { preview(img, screen) }
        if args.raw { answer(png(img)) }
        else if args.printGeometry, let g = geom {
            answer(Data("\(Int(g.width.rounded())) \(Int(g.height.rounded())) \(Int(g.minX.rounded())) \(Int(g.minY.rounded()))\n".utf8))
        } else { answer(Data()) }
        log("screenshot: \(action.rawValue) \(img.width)×\(img.height)")
    }

    // full / screen: no UI
    private func direct(_ cfg: ScreenshotConfig, _ a: ShotArgs, _ screens: [NSScreen],
                        _ images: [CGDirectDisplayID: CGImage], reply: ((Data?) -> Void)?) {
        var img: CGImage?
        var screen: NSScreen?
        if a.mode == .screen {
            let s = a.screenNumber.flatMap { screens.indices.contains($0) ? screens[$0] : nil } ?? mouseScreen
            screen = s
            img = s?.displayID.flatMap { images[$0] }
        } else {
            img = stitch(screens, images)
            screen = mouseScreen
        }
        guard let img else { reply?(nil); return }
        lastOutput = ["outcome": a.mode.rawValue, "size": [img.width, img.height]]
        if let p = a.path { save(img, cfg, screen, to: p) }
        if a.clipboard || (a.path == nil && !a.raw && !a.pin) { copy(img, cfg, screen) }
        if a.pin, let s = screen {
            pin(img, frame: CGRect(x: s.frame.minX, y: s.frame.minY, width: CGFloat(img.width) / s.backingScaleFactor,
                                   height: CGFloat(img.height) / s.backingScaleFactor), cfg)
        }
        reply?(a.raw ? png(img) : Data())
    }

    // every display at its global place, at the largest backing scale
    private func stitch(_ screens: [NSScreen], _ images: [CGDirectDisplayID: CGImage]) -> CGImage? {
        let union = screens.reduce(CGRect.null) { $0.union($1.frame) }
        guard !union.isNull else { return nil }
        let scale = screens.map(\.backingScaleFactor).max() ?? 2
        guard let ctx = CGContext(data: nil, width: Int(union.width * scale), height: Int(union.height * scale),
                                  bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .high
        for s in screens {
            guard let id = s.displayID, let img = images[id] else { continue }
            // AppKit frames are bottom-left like a CGContext: offset from the union
            let r = CGRect(x: (s.frame.minX - union.minX) * scale, y: (s.frame.minY - union.minY) * scale,
                           width: s.frame.width * scale, height: s.frame.height * scale)
            ctx.draw(img, in: r)
        }
        return ctx.makeImage()
    }

    func png(_ img: CGImage) -> Data? {
        NSBitmapImageRep(cgImage: img).representation(using: .png, properties: [:])
    }

    // ONE pasteboard item: PNG + TIFF (older apps)
    func copy(_ img: CGImage, _ cfg: ScreenshotConfig, _ screen: NSScreen?) {
        let rep = NSBitmapImageRep(cgImage: img)
        guard let png = rep.representation(using: .png, properties: [:]) else { return }
        let item = NSPasteboardItem()
        item.setData(png, forType: .png)
        if let tiff = rep.tiffRepresentation { item.setData(tiff, forType: .tiff) }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects([item])
        onOwnPasteboardWrite?()
        lastOutput["copied"] = true
        ScreenToast.show(cfg.copyToast, on: screen)
    }

    // Copy Text: Vision off the main thread (≈ 100-300 ms; the overlay is
    // already gone), then ONE string item + the toast. -r answers the text.
    private func deliverText(_ img: CGImage, _ cfg: ScreenshotConfig, _ args: ShotArgs, _ screen: NSScreen?) {
        let r = reply
        reply = nil
        let t0 = DispatchTime.now().uptimeNanoseconds
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let text = ShotOCR.text(img, cfg.ocr)
            DispatchQueue.main.async {
                guard let self else { return }
                let ms = Int(Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000)
                self.lastOutput = ["outcome": "text", "size": [img.width, img.height], "chars": text.count,
                                   "text": String(text.prefix(4000)), "ms": ms]
                if text.isEmpty {
                    ScreenToast.show(cfg.noTextToast, on: screen, symbol: "exclamationmark.triangle.fill")
                } else {
                    self.copyText(text, cfg, screen)
                }
                r?(args.raw ? Data(text.utf8) : Data())
                self.log("screenshot: text \(text.count) chars from \(img.width)×\(img.height), \(ms) ms")
            }
        }
    }

    func copyText(_ text: String, _ cfg: ScreenshotConfig, _ screen: NSScreen?) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        onOwnPasteboardWrite?()
        lastOutput["copied"] = true
        if !cfg.textToast.isEmpty {
            ScreenToast.show(cfg.textToast.replacingOccurrences(of: "{}", with: ShotOCR.summary(text)), on: screen,
                             symbol: "text.viewfinder")
        }
    }

    // to `path` (a directory → the pattern inside it), else save-path
    func save(_ img: CGImage, _ cfg: ScreenshotConfig, _ screen: NSScreen?, to path: String?) {
        let dir = ((path ?? cfg.savePath) as NSString).expandingTildeInPath
        if path == nil { try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true) }
        let target = ShotFiles.target(dir, pattern: cfg.filenamePattern, format: cfg.saveFormat)
        try? FileManager.default.createDirectory(atPath: (target as NSString).deletingLastPathComponent,
                                                 withIntermediateDirectories: true)
        write(img, to: target, cfg, screen)
    }

    private func write(_ img: CGImage, to path: String, _ cfg: ScreenshotConfig, _ screen: NSScreen?) {
        let rep = NSBitmapImageRep(cgImage: img)
        let ext = (path as NSString).pathExtension.lowercased()
        let data = ["jpg", "jpeg"].contains(ext)
            ? rep.representation(using: .jpeg, properties: [.compressionFactor: Double(cfg.jpegQuality) / 100])
            : rep.representation(using: .png, properties: [:])
        guard let data, (try? data.write(to: URL(fileURLWithPath: path), options: .atomic)) != nil else {
            ScreenToast.show("Could not save \((path as NSString).abbreviatingWithTildeInPath)", on: screen, symbol: "exclamationmark.triangle.fill")
            log("screenshot: save failed: \(path)")
            return
        }
        lastOutput["path"] = path
        onSaved?(path)
        if cfg.copyPathAfterSave {
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.setString(path, forType: .string)
            onOwnPasteboardWrite?()
        }
        ScreenToast.show(cfg.saveToast.replacingOccurrences(of: "{}", with: (path as NSString).abbreviatingWithTildeInPath), on: screen)
        log("screenshot: saved \(path)")
    }

    // the file browser's preview popup (FilePopup), sized in points
    func preview(_ img: CGImage, _ screen: NSScreen?, label: String = "Screenshot") {
        let scale = (screen ?? mouseScreen)?.backingScaleFactor ?? 2
        let ns = NSImage(cgImage: img, size: NSSize(width: CGFloat(img.width) / scale,
                                                    height: CGFloat(img.height) / scale))
        FilePopup.show(image: ns, label: label, over: nil, on: screen ?? mouseScreen)
    }

    func pin(_ img: CGImage, frame: CGRect?, _ cfg: ScreenshotConfig) {
        let scale = mouseScreen?.backingScaleFactor ?? 2
        let size = CGSize(width: CGFloat(img.width) / scale, height: CGFloat(img.height) / scale)
        var f = frame ?? CGRect(origin: .zero, size: size)
        if frame == nil, let s = mouseScreen { f.origin = CGPoint(x: s.frame.midX - size.width / 2, y: s.frame.midY - size.height / 2) }
        let p = PinPanel(image: img, frame: f, ui: cfg.uiColor, contrast: cfg.contrastColor)
        p.onClose = { [weak self] p in
            self?.pins.removeAll { $0 === p }
            self?.dropKeyMonitorIfIdle()
        }
        p.onCopy = { [weak self] img in self?.copy(img, ScreenshotConfig.load(), p.screen) }
        p.onSave = { [weak self] img in
            self?.save(img, ScreenshotConfig.load(), p.screen, to: nil)
        }
        pins.append(p)
        installKeyMonitor()
        p.present()
    }

    // MARK: tests (socket do:screenshot:… / state)

    func testDo(_ a: String) -> String? {
        let parts = a.split(separator: ":", maxSplits: 1).map(String.init)
        let verb = parts.first ?? ""
        let arg = parts.count > 1 ? parts[1] : ""
        func nums(_ s: String) -> [CGFloat] { s.split(separator: ",").compactMap { Double($0) }.map { CGFloat($0) } }
        switch verb {
        case "show", "show-text":
            var args = ShotArgs()
            if verb == "show-text" { args.mode = .text }
            args.delayMs = Int(arg) ?? 0
            trigger(args)
        case "select":
            let n = nums(arg)
            guard n.count == 4, let s = session else { return "select:X,Y,W,H (overlay up)" }
            s.testSelect(CGRect(x: n[0], y: n[1], width: n[2], height: n[3]))
        case "tool":
            guard let s = session else { return "no overlay" }
            if arg == "none" { s.forceTool(nil) }
            else {
                guard let t = ShotTool(rawValue: arg), t.isDrawing else { return "unknown tool \(arg)" }
                s.forceTool(t)
            }
        case "draw":
            let n = nums(arg)
            guard n.count == 4, let s = session else { return "draw:X1,Y1,X2,Y2 (overlay + tool)" }
            s.testDraw(CGPoint(x: n[0], y: n[1]), CGPoint(x: n[2], y: n[3]))
        case "key":
            guard let s = session, let e = Self.keyEvent(arg, window: s.mouseDisplay?.panel) else { return "key:SPEC (overlay up)" }
            _ = s.handleKey(e)
        case "copy": session?.finish(.copy)
        case "text": session?.finish(.text)
        case "mode":
            guard let s = session, arg == "text" || arg == "screenshot" else { return "mode:text|screenshot (overlay up)" }
            if s.textMode != (arg == "text") { s.toggleTextMode() }
        case "accept": session?.finish(.accept)
        case "pin": session?.finish(.pin)
        case "save":
            guard !arg.isEmpty else { return "save:PATH" }
            forcedSavePath = arg
            session?.finish(.save)
        case "save-ok":
            // the save card's Return (its field's action)
            guard let f = session?.saveCard?.card.field else { return "no save card" }
            f.sendAction(f.action, to: f.target)
        case "close":
            session?.finish(.abort)
        case "unpin":
            for p in pins { p.closePin() }
        case "side-panel":
            session?.toggleSidePanel()
        default:
            return "show[:MS] | show-text[:MS] | mode:text|screenshot | text | select:X,Y,W,H | tool:NAME | draw:X1,Y1,X2,Y2 | key:SPEC | copy | accept | save:PATH | save-ok | pin | unpin | close | side-panel"
        }
        return nil
    }

    // "cmd+shift+z", "esc", "return", "left", "p", "space"
    static func keyEvent(_ spec: String, window: NSWindow?) -> NSEvent? {
        var mods: NSEvent.ModifierFlags = []
        var key = ""
        for part in spec.lowercased().split(separator: "+").map(String.init) {
            switch part {
            case "cmd": mods.insert(.command)
            case "ctrl": mods.insert(.control)
            case "shift": mods.insert(.shift)
            case "opt", "alt": mods.insert(.option)
            default: key = part
            }
        }
        let named: [String: (UInt16, String)] = [
            "esc": (53, "\u{1b}"), "return": (36, "\r"), "delete": (51, "\u{7f}"), "space": (49, " "),
            "left": (123, "\u{F702}"), "right": (124, "\u{F703}"), "down": (125, "\u{F701}"), "up": (126, "\u{F700}"),
            "/": (44, "/"),
        ]
        let letters: [Character: UInt16] = ["a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
                                            "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "o": 31,
                                            "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46]
        var code: UInt16, chars: String
        if let n = named[key] { (code, chars) = n }
        else if key.count == 1, let c = key.first, let k = letters[c] { code = k; chars = key }
        else { return nil }
        return NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: mods, timestamp: ProcessInfo.processInfo.systemUptime,
                                windowNumber: window?.windowNumber ?? 0, context: nil,
                                characters: mods.contains(.shift) ? chars.uppercased() : chars,
                                charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)
    }

    var testState: [String: Any] {
        var st: [String: Any] = ["shown": session != nil, "permission": Self.permitted, "pins": pins.count,
                                 "last": lastOutput, "capturing": capturing]
        st["pinWids"] = pins.map(\.windowNumber)
        st["pinStates"] = pins.map { p -> [String: Any] in
            ["wid": p.windowNumber, "alpha": (Double(p.alphaValue) * 100).rounded() / 100, "key": p.isKeyWindow,
             "frame": [p.frame.minX, p.frame.minY, p.frame.width, p.frame.height].map { Int($0) }]
        }
        guard let s = session else { return st }
        st["displays"] = s.displays.map { d -> [String: Any] in
            let f = d.screen.frame
            return ["id": Int(d.id), "frame": [f.minX, f.minY, f.width, f.height].map { Int($0) },
                    "scale": d.canvas.scale, "key": d.panel.isKeyWindow, "wid": d.panel.windowNumber,
                    "visible": d.panel.isVisible, "level": d.panel.level.rawValue]
        }
        if let a = s.active, let r = s.selection {
            st["selection"] = ["display": Int(a.id), "x": r.minX, "y": r.minY, "w": r.width, "h": r.height]
            st["buttons"] = a.view.ringFrames.map { t, f in ["name": t.rawValue, "x": f.minX, "y": f.minY, "w": f.width, "h": f.height] }
        } else {
            st["selection"] = NSNull()
            st["buttons"] = []
        }
        st["tool"] = s.tool?.rawValue ?? NSNull()
        st["moveMode"] = s.moveMode
        st["textMode"] = s.textMode
        if let p = (s.mouseDisplay ?? s.displays.first)?.view.modePillFrame {
            st["modePill"] = ["x": p.minX, "y": p.minY, "w": p.width, "h": p.height]
        }
        st["size"] = s.activeSize ?? NSNull()
        st["color"] = s.currentColor.hex
        st["objects"] = s.doc.objects.map { o -> [String: Any] in
            let b = o.bbox
            var x: [String: Any] = ["type": o.tool.rawValue, "bbox": [b.minX, b.minY, b.width, b.height].map { Double($0) },
                                    "color": o.color.hex, "size": o.size]
            if o.tool == .counter { x["number"] = o.number }
            return x
        }
        st["selected"] = s.doc.selected ?? NSNull()
        st["canUndo"] = s.doc.canUndo
        st["canRedo"] = s.doc.canRedo
        st["sidePanel"] = s.sidePanelOpen
        st["helpShown"] = s.displays.contains { $0.view.helpShown }
        st["editingText"] = s.editing != nil
        st["wheel"] = s.wheel != nil
        st["grabbing"] = s.grabbing != nil
        st["saveCard"] = s.saveCard.map { ["path": $0.card.field.stringValue, "key": $0.display.panel.isKeyWindow] } ?? NSNull()
        return st
    }
}

extension ShotSession {
    // tests: set (not toggle) the tool
    func forceTool(_ t: ShotTool?) {
        if tool != t { setTool(t) }
    }
}


// MARK: - /pane-shot (PaneShot.swift)

// a herdr pane's scrollback + screen as ONE tall image, delivered like a
// capture: copy (one PNG + TIFF item) and save (→ /paths), one toast
extension ScreenshotController {
    struct PaneShotImage {
        let image: CGImage
        let rows: Int
        let title: String
        let pane: String?
    }

    // `pane-shot [flags]` from the socket / CLI. `reply` gets ONE line: the
    // saved path, "copied", or "error: …".
    func paneShot(_ words: [String], reply: ((String) -> Void)? = nil) {
        let args: PaneShotArgs
        switch PaneShotArgs.parse(words) {
        case .success(let a): args = a
        case .failure(let p):
            reply?("error: \(p.message)")
            return
        }
        var e: [String: String] = [:]
        for x in configSectionEntries(readConfigText().map(configLines) ?? [], "pane-shot") { e[x.key] = x.value }
        let cfg = PaneShotConfig(e)
        let screen = mouseScreen
        let scale = screen?.backingScaleFactor ?? 2
        let start = Date()
        // herdr + ghostty + the render (~50-300 ms) stay off the main thread
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Self.paneShotImage(args, cfg, scale: scale)
            DispatchQueue.main.async {
                guard let self else { return }
                switch result {
                case .failure(let f):
                    self.paneShotLast = ["error": f.message]
                    self.log("pane-shot: \(f.message)")
                    ScreenToast.show(f.message, on: screen, symbol: "exclamationmark.triangle.fill")
                    reply?("error: \(f.message)")
                case .success(let shot):
                    reply?(self.deliverPaneShot(shot, args, cfg, screen, since: start))
                }
            }
        }
    }

    private func deliverPaneShot(_ shot: PaneShotImage, _ args: PaneShotArgs, _ cfg: PaneShotConfig,
                                 _ screen: NSScreen?, since start: Date) -> String {
        let copy = args.copy ?? cfg.copy, save = args.save ?? cfg.save
        let toast = cfg.toast.replacingOccurrences(of: "{n}", with: String(shot.rows))
            .replacingOccurrences(of: "{pane}", with: shot.title)
        var sc = ScreenshotConfig.load()
        sc.copyPathAfterSave = false          // the image is the clipboard's
        sc.filenamePattern = cfg.filenamePattern
        sc.saveFormat = "png"
        if !cfg.savePath.isEmpty { sc.savePath = cfg.savePath }
        lastOutput["path"] = nil
        // save FIRST: the clipboard then carries the file too. One toast:
        // the copy's, else the save's
        if save {
            sc.saveToast = copy ? "" : "Saved {}"
            self.save(shot.image, sc, screen, to: nil)
        }
        let path = save ? lastOutput["path"] as? String : nil
        if copy { copyImage(shot.image, file: path, toast: toast, screen) }
        if cfg.preview { preview(shot.image, screen, label: shot.title) }
        let ms = Int(Date().timeIntervalSince(start) * 1000)
        paneShotLast = ["pane": shot.pane ?? "", "title": shot.title, "rows": shot.rows,
                        "size": [shot.image.width, shot.image.height], "copied": copy, "path": path ?? "", "ms": ms]
        log("pane-shot \(shot.pane ?? "file"): \(shot.rows) rows, \(shot.image.width)×\(shot.image.height) px, \(ms) ms")
        if save && path == nil { return "error: could not save" }
        return path ?? "copied"
    }

    // ONE pasteboard item: the PNG (image editors, Claude Code's Ctrl+V)
    // + the saved file's URL (chat / mail apps attach the FILE — they
    // shrink or drop a tall pasted bitmap). TIFF only for small images: a
    // 1000-row capture's TIFF is ~100 MB.
    private func copyImage(_ img: CGImage, file: String?, toast: String, _ screen: NSScreen?) {
        let rep = NSBitmapImageRep(cgImage: img)
        guard let png = rep.representation(using: .png, properties: [:]) else { return }
        let item = NSPasteboardItem()
        item.setData(png, forType: .png)
        if let file { item.setString(URL(fileURLWithPath: file).absoluteString, forType: .fileURL) }
        if img.width * img.height <= 4_000_000, let tiff = rep.tiffRepresentation { item.setData(tiff, forType: .tiff) }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects([item])
        onOwnPasteboardWrite?()
        ScreenToast.show(toast, on: screen)
    }

    // the pane's text (or --file) → Ghostty's theme → the image
    static func paneShotImage(_ args: PaneShotArgs, _ cfg: PaneShotConfig, scale: CGFloat) -> Result<PaneShotImage, Herdr.Failure> {
        let text: String, title: String, pane: String?
        if let file = args.file {
            let p = (file as NSString).expandingTildeInPath
            guard let t = try? String(contentsOfFile: p, encoding: .utf8) else {
                return .failure(Herdr.Failure(message: "can't read \(file)"))
            }
            (text, title, pane) = (t, (p as NSString).lastPathComponent, nil)
        } else {
            switch Herdr.pane(cfg.herdrBin, id: args.pane) {
            case .failure(let f): return .failure(f)
            case .success(let p):
                let n = Herdr.lines(viewport: p.viewportRows, history: args.lines ?? cfg.lines, all: args.all)
                switch Herdr.read(cfg.herdrBin, id: p.id, lines: n) {
                case .failure(let f): return .failure(f)
                case .success(let t): (text, title, pane) = (t, p.title, p.id)
                }
            }
        }
        let grid = AnsiGrid.parse(text)
        guard !grid.rows.isEmpty else { return .failure(Herdr.Failure(message: "nothing to capture: \(title) is empty")) }
        var theme = AnsiTheme()
        let ghostty = (cfg.ghosttyBin as NSString).expandingTildeInPath
        if FileManager.default.isExecutableFile(atPath: ghostty),
           let r = try? runProcess(ghostty, ["+show-config"]), r.code == 0 {
            theme = AnsiTheme.ghostty(r.out)
        }
        if !cfg.font.isEmpty { theme.fontName = cfg.font }
        if cfg.fontSize > 0 { theme.fontSize = CGFloat(cfg.fontSize) }
        if let bg = AnsiRGB(hex: cfg.background) { theme.background = bg }
        guard let img = AnsiRender.image(grid, theme: theme, padding: CGFloat(cfg.padding), scale: scale) else {
            return .failure(Herdr.Failure(message: "could not render \(grid.rows.count) rows"))
        }
        return .success(PaneShotImage(image: img, rows: grid.rows.count, title: title, pane: pane))
    }
}
