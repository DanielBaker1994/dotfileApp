import AppKit

enum ShotOutcome: String { case copy, save, pin, accept, abort, text }

final class ShotOverlayPanel: NSPanel {
    let overlay: ShotOverlayView
    private let backdrop = ShotBackdropView()

    init(screen: NSScreen) {
        overlay = ShotOverlayView(frame: NSRect(origin: .zero, size: screen.frame.size))
        super.init(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .screenSaver
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        hidesOnDeactivate = false
        isOpaque = true
        hasShadow = false
        backgroundColor = .black
        isReleasedWhenClosed = false
        acceptsMouseMovedEvents = true
        becomesKeyOnlyIfNeeded = false
        animationBehavior = .none
        backdrop.frame = NSRect(origin: .zero, size: screen.frame.size)
        backdrop.autoresizingMask = [.width, .height]
        overlay.autoresizingMask = [.width, .height]
        backdrop.addSubview(overlay)
        contentView = backdrop
        fit(screen)
    }
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
    override func cancelOperation(_ sender: Any?) {}

    func fit(_ screen: NSScreen) {
        if frame != screen.frame { setFrame(screen.frame, display: false) }
    }
    func setImage(_ img: CGImage?, scale: CGFloat) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        backdrop.layer?.contents = img
        backdrop.layer?.contentsScale = scale
        CATransaction.commit()
    }
}

final class ShotBackdropView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.contentsGravity = .resize
        layer?.backgroundColor = NSColor.black.cgColor
    }
    required init?(coder: NSCoder) { fatalError() }
}

final class ShotDisplay {
    let screen: NSScreen
    let id: CGDirectDisplayID
    let canvas: ShotCanvas
    let panel: ShotOverlayPanel
    var view: ShotOverlayView { panel.overlay }
    var bounds: CGRect { CGRect(origin: .zero, size: screen.frame.size) }

    init(screen: NSScreen, id: CGDirectDisplayID, canvas: ShotCanvas, panel: ShotOverlayPanel) {
        self.screen = screen; self.id = id; self.canvas = canvas; self.panel = panel
    }
    var globalOrigin: CGPoint {
        let primaryH = NSScreen.screens.first?.frame.maxY ?? screen.frame.maxY
        return CGPoint(x: screen.frame.minX, y: primaryH - screen.frame.maxY)
    }
}

final class ShotSession {
    let cfg: ScreenshotConfig
    let args: ShotArgs
    var state: ShotState
    let statePath: String
    let doc: ShotDocument
    var displays: [ShotDisplay] = []
    private(set) var active: ShotDisplay?
    private(set) var selection: CGRect?
    private(set) var tool: ShotTool?
    private(set) var moveMode = false
    private(set) var textMode = false
    private(set) var color: ShotColor
    private(set) var counterOffset = 0
    var sidePanelOpen: Bool { active.map { $0.view.sidePanel != nil } ?? false }
    private(set) var grabbing: ShotColor?
    private(set) var editing: (view: ShotTextField, index: Int?, display: ShotDisplay)?
    private(set) var wheel: (display: ShotDisplay, center: CGPoint, hot: Int?)?
    private(set) var shortcutsShown = false
    private(set) var saveCard: (card: ShotSaveCard, display: ShotDisplay)?
    var chosenSavePath: String?
    var finished = false
    var onFinish: ((ShotOutcome) -> Void)?
    var onRecent: (() -> Void)?
    var log: (String) -> Void = { _ in }
    private var ringNew = true
    private var lastWheel = Date.distantPast
    private var sizeSaveTimer: Timer?

    private enum Drag {
        case none
        case selecting(start: CGPoint)
        case resizing(handle: Int, from: CGRect, start: CGPoint)
        case movingSelection(from: CGRect, start: CGPoint)
        case drawing(ShotObject)
        case movingObject(index: Int, from: ShotObject, start: CGPoint, moved: Bool)
    }
    private var drag = Drag.none
    private(set) var mouse: (display: ShotDisplay, point: CGPoint)?

    init(cfg: ScreenshotConfig, args: ShotArgs, statePath: String) {
        self.cfg = cfg
        self.args = args
        self.statePath = statePath
        state = ShotState.load(statePath)
        doc = ShotDocument(undoLimit: cfg.undoLimit)
        color = state.color.flatMap { ShotColor(hex: $0) } ?? cfg.drawColor
        if state.style.family.isEmpty { state.style.family = cfg.font }
        textMode = args.mode == .text || (args.mode == .gui && cfg.startText)
    }

    var drawing: ShotObject? { if case .drawing(let o) = drag { return o }; return nil }
    var isDragging: Bool { if case .none = drag { return false }; return true }
    func size(_ t: ShotTool) -> Int { state.size(t) }
    var activeSize: Int? { tool.map { size($0) } }
    var currentColor: ShotColor {
        if let i = doc.selected, doc.objects.indices.contains(i) { return doc.objects[i].color }
        return color
    }

    func display(at globalMouse: NSPoint) -> ShotDisplay? {
        displays.first { NSMouseInRect(globalMouse, $0.screen.frame, false) }
    }
    var mouseDisplay: ShotDisplay? { display(at: NSEvent.mouseLocation) ?? displays.first }

    func redrawAll() {
        for d in displays { d.view.needsDisplay = true; d.view.layoutChrome() }
    }

    func select(_ r: CGRect?, on d: ShotDisplay?, new: Bool = true) {
        let old = (active, selection)
        if let d, d !== active {
            doc.reset()
            active = d
        }
        if r == nil { active = d ?? active }
        selection = r.map { $0.standardized.intersection(active?.bounds ?? $0) }
        if let s = selection, s.isNull || s.width < 0.5 || s.height < 0.5 { selection = nil }
        if new { ringNew = true }
        if let od = old.0 { od.view.invalidate(old.1) }
        active?.view.invalidate(selection)
        for d in displays { d.view.layoutChrome() }
    }

    func takeRingNew() -> Bool {
        defer { ringNew = false }
        return ringNew
    }

    func selectAll() {
        guard let d = mouseDisplay else { return }
        select(d.bounds, on: d)
    }

    var globalSelection: CGRect? {
        guard let d = active, let s = selection else { return nil }
        let o = d.globalOrigin
        return s.offsetBy(dx: o.x, dy: o.y)
    }

    func setTool(_ t: ShotTool?) {
        commitText()
        moveMode = false
        tool = (t == tool) ? nil : t
        doc.selected = nil
        redrawAll()
    }
    func toggleTextMode() {
        commitText()
        closeWheel()
        if grabbing != nil { cancelGrab() }
        if sidePanelOpen { toggleSidePanel(open: false) }
        if shortcutsShown { hideShortcuts() }
        textMode.toggle()
        tool = nil
        moveMode = false
        doc.selected = nil
        for d in displays { d.view.invalidate(nil) }
        redrawAll()
    }

    func toggleMoveMode() {
        commitText()
        moveMode.toggle()
        if moveMode { tool = nil }
        redrawAll()
    }

    func setColor(_ c: ShotColor) {
        if let i = doc.selected, doc.objects.indices.contains(i) {
            doc.update(at: i) { $0.color = c }
        } else {
            color = c
            state.color = c.hex
            state.save(statePath)
        }
        if let e = editing { e.view.textColor = NSColor(cgColor: c.cgColor) }
        redrawAll()
    }

    func changeSize(_ delta: Int) {
        if let i = doc.selected, doc.objects.indices.contains(i) {
            let t = doc.objects[i].tool
            doc.update(at: i, coalesce: "size\(i)") { $0.size = max(t.sizeRange.lowerBound, min(t.sizeRange.upperBound, $0.size + delta)) }
            showSize(doc.objects[i].size)
            redrawAll()
            return
        }
        guard let t = tool else { return }
        let v = max(t.sizeRange.lowerBound, min(t.sizeRange.upperBound, size(t) + delta))
        state.sizes[t.rawValue] = v
        showSize(v)
        if let e = editing { e.view.applyStyle(size: v) }
        sizeSaveTimer?.invalidate()
        sizeSaveTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: false) { [weak self] _ in
            guard let self else { return }
            self.state.save(self.statePath)
        }
        redrawAll()
    }
    func setSize(_ v: Int) {
        guard let t = tool ?? doc.selected.map({ doc.objects[$0].tool }) else { return }
        changeSize(v - (doc.selected.map { doc.objects[$0].size } ?? size(t)))
    }

    private func showSize(_ v: Int) {
        (active ?? mouseDisplay)?.view.showSizeIndicator(v)
    }

    func newObject(_ t: ShotTool, at p: CGPoint) -> ShotObject {
        var o = ShotObject(tool: t, points: [p, p], color: color, size: size(t))
        o.openArrow = cfg.arrowStyle == 1
        o.reversed = cfg.reverseArrow
        o.outline = cfg.counterOutline
        o.secure = !cfg.insecurePixelate
        o.style = state.style
        if t == .counter {
            o.points = [p]
            o.numberOffset = counterOffset
        }
        if t == .pencil { o.points = [p] }
        return o
    }

    private func handleAt(_ p: CGPoint) -> Int? {
        guard let s = selection else { return nil }
        let r = max(6, cfg.buttonSize * 0.3) + 2
        for (i, h) in ShotOverlayView.handlePoints(s).enumerated() where ShotGeom.dist(p, h) <= r { return i }
        return nil
    }

    func mouseDown(_ e: NSEvent, _ d: ShotDisplay, _ p: CGPoint) {
        mouse = (d, p)
        if shortcutsShown { hideShortcuts(); return }
        if let w = wheel {
            if let hot = w.display.view.wheelIndex(at: p, center: w.center, on: d === w.display) { pickWheel(hot) }
            closeWheel()
            return
        }
        if grabbing != nil {
            if let c = d.canvas.color(at: p) { grabbing = nil; setColor(c) }
            d.view.hideLoupe()
            redrawAll()
            return
        }
        if editing != nil { commitText(); return }
        if textMode {
            drag = .selecting(start: p)
            select(CGRect(origin: p, size: .zero), on: d)
            d.view.ringHidden = true
            redrawAll()
            return
        }
        let inActive = d === active
        if e.clickCount == 2, inActive, let s = selection, s.contains(p) {
            if tool == .text || tool == nil, let i = doc.hit(p), doc.objects[i].tool == .text {
                beginText(d, at: doc.objects[i].start, editing: i)
                return
            }
            if cfg.copyOnDoubleClick && tool == nil { finish(.copy); return }
        }
        if inActive, let h = handleAt(p) {
            drag = .resizing(handle: h, from: selection!, start: p)
            d.view.ringHidden = true
            return
        }
        if inActive, moveMode, let s = selection {
            drag = .movingSelection(from: s, start: p)
            d.view.ringHidden = true
            return
        }
        if inActive, selection != nil, let i = doc.hit(p) {
            doc.selected = i
            drag = .movingObject(index: i, from: doc.objects[i], start: p, moved: false)
            redrawAll()
            return
        }
        doc.selected = nil
        if let t = tool, inActive, selection != nil {
            if t == .text { beginText(d, at: CGPoint(x: p.x, y: p.y - (ShotText.metrics(newObject(.text, at: p)).lineHeight / 2) - ShotText.padding), editing: nil); return }
            drag = .drawing(newObject(t, at: p))
            d.view.ringHidden = true
            d.view.invalidate(CGRect(origin: p, size: .zero).insetBy(dx: -40, dy: -40))
            return
        }
        if inActive, tool == nil, let s = selection, s.contains(p) {
            drag = .movingSelection(from: s, start: p)
            d.view.ringHidden = true
            return
        }
        drag = .selecting(start: p)
        select(CGRect(origin: p, size: .zero), on: d)
        d.view.ringHidden = true
        redrawAll()
    }

    func mouseDragged(_ e: NSEvent, _ d: ShotDisplay, _ p: CGPoint) {
        mouse = (d, p)
        let shift = e.modifierFlags.contains(.shift), cmd = e.modifierFlags.contains(.command)
        switch drag {
        case .none: break
        case .selecting(let start):
            var q = p
            if shift { q = ShotSnap.square(from: start, to: p) }
            select(ShotGeom.rect(start, q), on: d, new: true)
        case .resizing(let h, let from, let start):
            select(Self.resize(from, handle: h, by: CGVector(dx: p.x - start.x, dy: p.y - start.y),
                               mirror: shift, keepAspect: cmd), on: active, new: true)
        case .movingSelection(let from, let start):
            guard let a = active else { break }
            var r = from.offsetBy(dx: p.x - start.x, dy: p.y - start.y)
            r.origin.x = max(0, min(r.minX, a.bounds.width - r.width))
            r.origin.y = max(0, min(r.minY, a.bounds.height - r.height))
            select(r, on: a, new: false)
        case .drawing(var o):
            let before = o.bbox
            switch o.tool {
            case .pencil:
                if let last = o.points.last, ShotGeom.dist(last, p) >= 1 { o.points.append(p) }
            case .counter:
                o.points = [o.start, p]
            default:
                o.points = [o.start, shift ? ShotSnap.snap(o.tool, from: o.start, to: p) : p]
            }
            drag = .drawing(o)
            active?.view.invalidate(before.union(o.bbox))
        case .movingObject(let i, let from, let start, _):
            let dv = CGVector(dx: p.x - start.x, dy: p.y - start.y)
            let before = doc.objects[i].bbox
            doc.updateLive(at: i) { $0 = from.moved(by: dv) }
            drag = .movingObject(index: i, from: from, start: start, moved: true)
            active?.view.invalidate(before.union(doc.objects[i].bbox))
        }
    }

    func mouseUp(_ e: NSEvent, _ d: ShotDisplay, _ p: CGPoint) {
        let was = drag
        drag = .none
        switch was {
        case .none: break
        case .selecting:
            if let s = selection, s.width < 2 || s.height < 2 { select(nil, on: active) }
            if textMode, selection != nil { finish(.text); return }
            if args.acceptOnSelect, selection != nil { finish(.accept); return }
        case .resizing, .movingSelection: break
        case .drawing(var o):
            let r = o.rect
            switch o.tool {
            case .pencil, .counter:
                if o.tool == .counter, o.points.count > 1, ShotGeom.dist(o.start, o.end) < o.counterRadius {
                    o.points = [o.start]
                }
                doc.add(o)
                if o.tool == .counter { counterOffset = 0 }
            default:
                if max(r.width, r.height) >= 2 { doc.add(o) }
            }
            active?.view.invalidate(o.bbox)
        case .movingObject(let i, let from, _, let moved):
            if moved {
                let now = doc.objects[i]
                doc.updateLive(at: i) { $0 = from }
                doc.update(at: i) { $0 = now }
                doc.selected = i
            }
        }
        for v in displays { v.view.ringHidden = false }
        redrawAll()
    }

    func mouseMoved(_ d: ShotDisplay, _ p: CGPoint) {
        let old = mouse
        mouse = (d, p)
        if !d.panel.isKeyWindow, editing == nil, saveCard == nil, !(NSApp.keyWindow?.firstResponder is NSText) {
            d.panel.makeKey()
        }
        if grabbing != nil || cfg.magnifier && selection == nil { d.view.showLoupe(at: p, grab: grabbing != nil) }
        else { d.view.hideLoupe() }
        if let o = old, o.display !== d { o.display.view.hideLoupe(); o.display.view.invalidate(dotRect(o.point)) }
        if let o = old, o.display === d { d.view.invalidate(dotRect(o.point)) }
        d.view.invalidate(dotRect(p))
        d.view.updateCursor(at: p)
        if wheel != nil, let w = wheel, w.display === d {
            wheel?.hot = d.view.wheelIndex(at: p, center: w.center, on: true)
            d.view.showWheel(center: w.center, hot: wheel?.hot)
        }
    }
    private func dotRect(_ p: CGPoint) -> CGRect {
        let r = CGFloat(max(size(.marker) * 2 + 2, 30))
        return CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)
    }

    func scroll(_ e: NSEvent, _ d: ShotDisplay) {
        if e.hasPreciseScrollingDeltas {
            guard Date().timeIntervalSince(lastWheel) >= 0.2, abs(e.scrollingDeltaY) > 0.5 else { return }
        }
        guard e.scrollingDeltaY != 0 else { return }
        lastWheel = Date()
        let step = e.scrollingDeltaY > 0 ? 1 : -1
        if e.modifierFlags.contains(.command), tool == .counter {
            counterOffset = max(-doc.nextCounterNumber() + 1, min(998, counterOffset + step))
            showSize(doc.nextCounterNumber(offset: counterOffset))
            return
        }
        changeSize(step)
    }

    func rightDown(_ d: ShotDisplay, _ p: CGPoint) {
        if editing != nil { commitText() }
        wheel = (d, p, nil)
        d.view.showWheel(center: p, hot: nil)
    }
    func rightDragged(_ d: ShotDisplay, _ p: CGPoint) {
        guard let w = wheel else { return }
        wheel?.hot = w.display.view.wheelIndex(at: p, center: w.center, on: d === w.display)
        w.display.view.showWheel(center: w.center, hot: wheel?.hot)
    }
    func rightUp(_ d: ShotDisplay, _ p: CGPoint) {
        guard let w = wheel else { return }
        if let hot = w.hot { pickWheel(hot); closeWheel() }
    }
    private func pickWheel(_ i: Int) {
        let list = cfg.userColors
        guard list.indices.contains(i) else { return }
        if let c = list[i] { setColor(c) } else { toggleSidePanel(open: true) }
    }
    func closeWheel() {
        wheel?.display.view.hideWheel()
        wheel = nil
    }

    func startGrab() {
        commitText()
        grabbing = color
        if let m = mouse { m.display.view.showLoupe(at: m.point, grab: true) }
    }
    func cancelGrab() {
        if let prev = grabbing { color = prev }
        grabbing = nil
        for d in displays { d.view.hideLoupe() }
        redrawAll()
    }

    func beginText(_ d: ShotDisplay, at origin: CGPoint, editing index: Int?) {
        commitText()
        var o = index.map { doc.objects[$0] } ?? newObject(.text, at: origin)
        if index == nil { o.points = [origin] }
        let tf = ShotTextField(object: o)
        tf.onChange = { [weak self] in self?.editing?.display.view.needsLayout = true }
        editing = (tf, index, d)
        d.view.addSubview(tf)
        tf.place()
        d.panel.makeKey()
        d.panel.makeFirstResponder(tf)
        if let i = index { doc.selected = nil; d.view.hiddenObject = i }
        redrawAll()
    }

    func commitText() {
        guard let e = editing else { return }
        editing = nil
        let text = e.view.string.trimmingCharacters(in: .newlines)
        var o = e.view.object
        o.text = text
        e.display.view.hiddenObject = nil
        e.view.removeFromSuperview()
        e.display.panel.makeFirstResponder(e.display.view)
        if let i = e.index {
            if text.isEmpty { doc.remove(at: i) } else { doc.update(at: i) { $0 = o } }
        } else if !text.isEmpty {
            doc.add(o)
        }
        redrawAll()
    }

    func setTextStyle(_ f: (inout ShotTextStyle) -> Void) {
        f(&state.style)
        state.save(statePath)
        if let e = editing {
            f(&e.view.object.style)
            e.view.applyStyle(size: nil)
        } else if let i = doc.selected, doc.objects.indices.contains(i), doc.objects[i].tool == .text {
            doc.update(at: i) { f(&$0.style) }
        }
        redrawAll()
    }

    func toggleSidePanel(open: Bool? = nil) {
        guard let d = active ?? mouseDisplay else { return }
        let want = open ?? (d.view.sidePanel == nil)
        for v in displays where v !== d { v.view.setSidePanel(nil) }
        d.view.setSidePanel(want ? self : nil)
    }

    func showShortcuts() {
        guard let d = mouseDisplay else { return }
        shortcutsShown = true
        d.view.showShortcuts(cfg.shortcuts)
    }
    func hideShortcuts() {
        shortcutsShown = false
        for d in displays { d.view.hideShortcuts() }
    }

    func nudge(_ dx: CGFloat, _ dy: CGFloat, resize: Bool, symmetric: Bool) {
        guard let a = active, var s = selection else { return }
        if symmetric {
            s = s.insetBy(dx: -dx, dy: -dy)
        } else if resize {
            s.size.width = max(1, s.width + dx)
            s.size.height = max(1, s.height + dy)
        } else {
            s = s.offsetBy(dx: dx, dy: dy)
            s.origin.x = max(0, min(s.minX, a.bounds.width - s.width))
            s.origin.y = max(0, min(s.minY, a.bounds.height - s.height))
        }
        select(s, on: a, new: false)
    }

    static func resize(_ r: CGRect, handle h: Int, by d: CGVector, mirror: Bool, keepAspect: Bool) -> CGRect {
        var minX = r.minX, minY = r.minY, maxX = r.maxX, maxY = r.maxY
        let left = [0, 6, 7].contains(h), right = [2, 3, 4].contains(h)
        let top = [0, 1, 2].contains(h), bottom = [4, 5, 6].contains(h)
        var dx = d.dx, dy = d.dy
        if keepAspect, r.width > 0, r.height > 0, (left || right), (top || bottom) {
            let ratio = r.width / r.height
            let sx = left ? -dx : dx, sy = top ? -dy : dy
            let s = max(sx, sy * ratio)
            dx = left ? -s : s
            dy = (top ? -1 : 1) * s / ratio
        }
        if left { minX += dx; if mirror { maxX -= dx } }
        if right { maxX += dx; if mirror { minX -= dx } }
        if top { minY += dy; if mirror { maxY -= dy } }
        if bottom { maxY += dy; if mirror { minY -= dy } }
        return ShotGeom.rect(CGPoint(x: minX, y: minY), CGPoint(x: maxX, y: maxY))
    }

    func handleKey(_ e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        let shift = mods.contains(.shift), opt = mods.contains(.option)
        let ch = (e.charactersIgnoringModifiers ?? "").lowercased()
        let code = e.keyCode
        let isEsc = code == 53
        let window = e.window ?? mouseDisplay?.panel

        if let w = window, w.firstResponder is NSText {
            if isEsc {
                if saveCard != nil { closeSaveCard() }
                else if editing != nil { commitText() }
                else { active?.view.sidePanel?.leaveField(); w.makeFirstResponder(active?.view ?? w.contentView) }
                return true
            }
            if editing != nil, cmd, code == 36 { commitText(); return true }
            if cmd, ch == "q" { finish(.abort); return true }
            if JiraEditKeys.route(e, in: w) { return true }
            return false
        }

        if isEsc { escape(); return true }
        if shortcutsShown { hideShortcuts(); return true }
        if cmd && ch == "q" { finish(.abort); return true }
        if cmd && ch == "/" { showShortcuts(); return true }
        if cmd && ch == "r" { pressed(.recent); return true }
        if code == 48 && !cmd && !ctrl && !opt { toggleTextMode(); return true }
        if cmd && shift && ch == "c" { finish(.text); return true }
        if textMode {
            if (cmd || ctrl) && ch == "c" { finish(.text); return true }
            if code == 36 || code == 76 { finish(.text); return true }
            if cmd && ch == "a" { selectAll(); finish(.text); return true }
            if !cmd && !ctrl && !opt && ch == "o" { toggleTextMode() }
            return true
        }
        if (cmd || ctrl) && ch == "c" && !shift { finish(.copy); return true }
        if cmd && ch == "s" { requestSave(); return true }
        if cmd && ch == "a" { selectAll(); return true }
        if cmd && ch == "m" { toggleMoveMode(); return true }
        if cmd && ch == "z" { shift ? redo() : undo(); return true }
        if cmd && code == 36 { commitText(); return true }
        if code == 36 || code == 76 { finish(.accept); return true }
        if code == 51 || code == 117 {
            if let i = doc.selected { doc.remove(at: i); redrawAll() }
            return true
        }
        let arrows: [UInt16: (CGFloat, CGFloat)] = [123: (-1, 0), 124: (1, 0), 125: (0, 1), 126: (0, -1)]
        if let (dx, dy) = arrows[code] {
            if cmd && shift { nudge(dx, dy, resize: true, symmetric: true) }
            else if shift { nudge(dx, dy, resize: true, symmetric: false) }
            else if !cmd { nudge(dx, dy, resize: false, symmetric: false) }
            return true
        }
        if !cmd && !ctrl && !opt {
            if ch == " " { toggleSidePanel(); return true }
            if ch == "g" { startGrab(); return true }
            if ch == "o" { toggleTextMode(); return true }
            if let c = ch.first, let t = ShotTool.forLetter(c) { setTool(t); return true }
        }
        return true
    }

    func escape() {
        if saveCard != nil { closeSaveCard(); return }
        if editing != nil { commitText(); return }
        if shortcutsShown { hideShortcuts(); return }
        if wheel != nil { closeWheel(); return }
        if grabbing != nil { cancelGrab(); return }
        if sidePanelOpen { toggleSidePanel(open: false); return }
        if doc.selected != nil { doc.selected = nil; redrawAll(); return }
        if tool != nil || moveMode { tool = nil; moveMode = false; redrawAll(); return }
        finish(.abort)
    }

    func undo() { commitText(); if doc.undo() { redrawAll() } }
    func redo() { commitText(); if doc.redo() { redrawAll() } }

    func pressed(_ t: ShotTool) {
        switch t {
        case _ where t.isDrawing: setTool(t)
        case .move: toggleMoveMode()
        case .undo: undo()
        case .redo: redo()
        case .copy: finish(.copy)
        case .save: requestSave()
        case .accept: finish(.accept)
        case .exit: finish(.abort)
        case .pin: finish(.pin)
        case .copyText: finish(.text)
        case .recent: onRecent?()
        case .sizeUp: changeSize(1)
        case .sizeDown: changeSize(-1)
        default: break
        }
    }

    func requestSave() {
        guard selection != nil else { return }
        if cfg.savePathFixed || args.path != nil { finish(.save); return }
        guard let d = active, saveCard == nil else { return }
        commitText()
        closeWheel()
        let dir = (cfg.savePath as NSString).expandingTildeInPath
        let path = ShotFiles.uniquePath(dir: dir, name: ShotFiles.expand(cfg.filenamePattern), ext: cfg.saveFormat)
        let card = ShotSaveCard(path: (path as NSString).abbreviatingWithTildeInPath, ui: cfg.uiColor) { [weak self] p in
            self?.confirmSave(p)
        }
        card.frame.origin = CGPoint(x: ((d.bounds.width - card.frame.width) / 2).rounded(),
                                    y: ((d.bounds.height - card.frame.height) / 2).rounded())
        d.view.addSubview(card)
        saveCard = (card, d)
        d.panel.makeKey()
        card.focus()
    }
    private func confirmSave(_ typed: String) {
        guard saveCard != nil else { return }
        let p = (typed.trimmingCharacters(in: .whitespaces) as NSString).expandingTildeInPath
        guard !p.isEmpty else { return }
        chosenSavePath = p
        closeSaveCard()
        finish(.save)
    }
    func closeSaveCard() {
        guard let c = saveCard else { return }
        saveCard = nil
        c.card.removeFromSuperview()
        c.display.panel.makeFirstResponder(c.display.view)
    }

    func finish(_ o: ShotOutcome) {
        guard !finished else { return }
        commitText()
        if o != .abort && o != .save && selection == nil { return }
        if o == .save && selection == nil { return }
        finished = true
        state.save(statePath)
        onFinish?(o)
    }

    func render(objects: Bool = true) -> CGImage? {
        guard let d = active, let s = selection else { return nil }
        return ShotRenderer.render(d.canvas, crop: s, objects: objects ? doc.objects : [])
    }

    func testDraw(_ a: CGPoint, _ b: CGPoint) {
        guard let d = active, let t = tool else { return }
        var o = newObject(t, at: a)
        if t == .text { o.text = "text"; o.points = [a] }
        else if t == .counter { o.points = a == b ? [a] : [a, b] }
        else if t == .pencil { o.points = [a, CGPoint(x: (a.x + b.x) / 2, y: a.y), b] }
        else { o.points = [a, b] }
        doc.add(o)
        if t == .counter { counterOffset = 0 }
        d.view.needsDisplay = true
    }
    func testSelect(_ r: CGRect, on display: ShotDisplay? = nil) {
        guard let d = display ?? mouseDisplay else { return }
        select(r, on: d)
        for v in displays { v.view.ringHidden = false }
        redrawAll()
    }
}

final class ShotOverlayView: NSView {
    weak var session: ShotSession?
    weak var display: ShotDisplay?
    var hiddenObject: Int?
    var ringHidden = false { didSet { if ringHidden != oldValue { layoutChrome() } } }
    private var buttons: [ShotButton] = []
    private var ringTools: [ShotTool] = []
    private var help: ShotHelpCard?
    private var helpIsText = false
    private var tab: ShotToolTab?
    private var modePill: ShotModePill?
    private var recentBtn: ShotRecentButton?
    private(set) var sidePanel: ShotSidePanel?
    private var indicator: ShotSizeIndicator?
    private var wheelView: ShotWheelView?
    private var loupe: ShotLoupe?
    private var shortcuts: ShotShortcutsCard?
    private var tracking: NSTrackingArea?

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
    }
    required init?(coder: NSCoder) { fatalError() }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeAlways, .inVisibleRect, .cursorUpdate], owner: self)
        addTrackingArea(t)
        tracking = t
    }

    func attach(_ s: ShotSession, _ d: ShotDisplay) {
        session = s
        display = d
        hiddenObject = nil
        ringHidden = false
        for v in subviews { v.removeFromSuperview() }
        buttons = []
        ringTools = []
        help = nil; tab = nil; modePill = nil; recentBtn = nil; sidePanel = nil; indicator = nil; wheelView = nil; loupe = nil; shortcuts = nil
        needsDisplay = true
        layoutChrome()
    }
    func detach() {
        for v in subviews { v.removeFromSuperview() }
        buttons = []
        help = nil; tab = nil; modePill = nil; recentBtn = nil; sidePanel = nil; indicator = nil; wheelView = nil; loupe = nil; shortcuts = nil
        session = nil
        display = nil
    }

    func invalidate(_ r: CGRect?) {
        guard let r else { needsDisplay = true; return }
        let pad = max(24, (session?.cfg.buttonSize ?? 34) * 0.4)
        setNeedsDisplay(r.insetBy(dx: -pad, dy: -pad))
    }

    static func handlePoints(_ s: CGRect) -> [CGPoint] {
        [CGPoint(x: s.minX, y: s.minY), CGPoint(x: s.midX, y: s.minY), CGPoint(x: s.maxX, y: s.minY),
         CGPoint(x: s.maxX, y: s.midY), CGPoint(x: s.maxX, y: s.maxY), CGPoint(x: s.midX, y: s.maxY),
         CGPoint(x: s.minX, y: s.maxY), CGPoint(x: s.minX, y: s.midY)]
    }

    override func draw(_ dirty: NSRect) {
        guard let s = session, let d = display, let ctx = NSGraphicsContext.current?.cgContext else { return }
        let cfg = s.cfg
        let mine = s.active === d ? s.selection : nil
        ctx.saveGState()
        ctx.setFillColor(NSColor.black.withAlphaComponent(CGFloat(cfg.contrastOpacity) / 255).cgColor)
        if let sel = mine {
            ctx.addRect(dirty)
            ctx.addRect(sel)
            ctx.fillPath(using: .evenOdd)
        } else {
            ctx.fill(dirty)
        }
        ctx.restoreGState()
        let modeColor: CGColor = s.textMode ? shotTextBlue.cgColor : cfg.uiColor.cgColor
        ctx.saveGState()
        ctx.setFillColor(modeColor.copy(alpha: 0.14) ?? modeColor)
        ctx.fill(dirty)
        ctx.setStrokeColor(modeColor.copy(alpha: 0.6) ?? modeColor)
        ctx.setLineWidth(8)
        ctx.stroke(bounds.insetBy(dx: 4, dy: 4))
        ctx.restoreGState()
        guard let sel = mine, sel.width > 0, sel.height > 0 else { return }

        ctx.saveGState()
        ctx.clip(to: sel.intersection(dirty))
        ShotRenderer.drawBase(d.canvas, sel.intersection(dirty), ctx)
        var objs = s.doc.objects
        if let h = hiddenObject, objs.indices.contains(h) { objs.remove(at: h) }
        ShotRenderer.draw(objs, d.canvas, ctx)
        if let o = s.drawing { ShotRenderer.draw(o, d.canvas, ctx) }
        if s.state.grid { drawGrid(ctx, sel, step: CGFloat(s.state.gridSize)) }
        ctx.restoreGState()

        if let i = s.doc.selected, s.doc.objects.indices.contains(i) {
            ctx.saveGState()
            ctx.setStrokeColor(cfg.uiColor.cgColor)
            ctx.setLineWidth(1)
            ctx.setLineDash(phase: 0, lengths: [4, 3])
            ctx.stroke(s.doc.objects[i].bbox.insetBy(dx: -4, dy: -4))
            ctx.restoreGState()
        }
        ctx.setStrokeColor(modeColor)
        if s.textMode {
            ctx.setLineWidth(2)
            ctx.setLineDash(phase: 0, lengths: [6, 4])
            ctx.stroke(sel.insetBy(dx: -1, dy: -1))
            ctx.setLineDash(phase: 0, lengths: [])
            return
        }
        ctx.setLineWidth(1)
        ctx.stroke(sel.insetBy(dx: -0.5, dy: -0.5))
        if !s.isDragging || ringHidden {
            let hd = (cfg.buttonSize * 0.6 * 0.5).rounded()
            ctx.setFillColor(modeColor)
            for p in Self.handlePoints(sel) {
                ctx.fillEllipse(in: CGRect(x: p.x - hd / 2, y: p.y - hd / 2, width: hd, height: hd))
            }
        }
        if let m = s.mouse, m.display === d, s.editing == nil, s.grabbing == nil, !s.isDragging,
           let t = s.tool, [.pencil, .marker, .line, .arrow].contains(t), sel.contains(m.point) {
            let probe = ShotObject(tool: t, points: [m.point], color: s.color, size: s.size(t))
            let w = probe.strokeWidth
            ctx.setFillColor((t == .marker ? s.color.with(alpha: 0.4) : s.color).cgColor)
            ctx.fillEllipse(in: CGRect(x: m.point.x - w / 2, y: m.point.y - w / 2, width: w, height: w))
        }
    }

    private func drawGrid(_ ctx: CGContext, _ r: CGRect, step: CGFloat) {
        guard step >= 2 else { return }
        ctx.setStrokeColor(NSColor.white.withAlphaComponent(0.18).cgColor)
        ctx.setLineWidth(0.5)
        var x = r.minX + step
        while x < r.maxX { ctx.move(to: CGPoint(x: x, y: r.minY)); ctx.addLine(to: CGPoint(x: x, y: r.maxY)); x += step }
        var y = r.minY + step
        while y < r.maxY { ctx.move(to: CGPoint(x: r.minX, y: y)); ctx.addLine(to: CGPoint(x: r.maxX, y: y)); y += step }
        ctx.strokePath()
    }

    func layoutChrome() {
        guard let s = session, let d = display else { return }
        let cfg = s.cfg
        let mine = s.active === d ? s.selection : nil
        let showHelp = cfg.showHelp && s.selection == nil && s.mouseDisplay === d
        if help != nil && helpIsText != s.textMode { help?.removeFromSuperview(); help = nil }
        if showHelp {
            if help == nil {
                let h = ShotHelpCard(rows: s.textMode ? cfg.helpTextRows : cfg.helpRows, ui: s.textMode ? shotTextBlue : cfg.uiColor)
                helpIsText = s.textMode
                addSubview(h, positioned: .below, relativeTo: nil)
                help = h
            }
            if let h = help {
                h.frame.origin = CGPoint(x: ((bounds.width - h.frame.width) / 2).rounded(),
                                         y: ((bounds.height - h.frame.height) / 2).rounded())
            }
        } else {
            help?.removeFromSuperview()
            help = nil
        }
        if s.mouseDisplay === d && !s.isDragging {
            if modePill == nil {
                let m = ShotModePill(ui: cfg.uiColor) { [weak s] text in
                    if let s, s.textMode != text { s.toggleTextMode() }
                }
                addSubview(m)
                modePill = m
            }
            let top = max(window?.screen.map { $0.frame.maxY - $0.visibleFrame.maxY } ?? 0, 24)
            if let m = modePill {
                m.textMode = s.textMode
                m.frame.origin = CGPoint(x: ((bounds.width - m.frame.width) / 2).rounded(), y: top + 16)
            }
            if recentBtn == nil {
                let r = ShotRecentButton(ui: cfg.uiColor) { [weak s] in s?.pressed(.recent) }
                addSubview(r)
                recentBtn = r
            }
            if let m = modePill, let r = recentBtn {
                r.frame.origin = CGPoint(x: ((bounds.width - m.frame.width) / 2 + m.frame.width + 12).rounded(), y: top + 19)
            }
        } else {
            modePill?.removeFromSuperview()
            modePill = nil
            recentBtn?.removeFromSuperview()
            recentBtn = nil
        }
        let showTab = !s.textMode && cfg.showSidePanelButton && sidePanel == nil && (mine != nil || (s.selection == nil && s.mouseDisplay === d))
        if showTab {
            if tab == nil {
                let t = ShotToolTab(ui: cfg.uiColor) { [weak s] in s?.toggleSidePanel(open: true) }
                addSubview(t)
                tab = t
            }
            tab?.frame.origin = CGPoint(x: 0, y: ((bounds.height - (tab?.frame.height ?? 0)) / 2).rounded())
        } else {
            tab?.removeFromSuperview()
            tab = nil
        }
        guard let sel = mine, !ringHidden, !s.textMode else {
            for b in buttons { b.isHidden = true }
            if mine == nil { buttons.forEach { $0.removeFromSuperview() }; buttons = [] }
            return
        }
        if ringTools != cfg.buttons || buttons.count != cfg.buttons.count {
            buttons.forEach { $0.removeFromSuperview() }
            ringTools = cfg.buttons
            buttons = cfg.buttons.map { t in
                let b = ShotButton(tool: t, size: cfg.buttonSize, ui: cfg.uiColor, contrast: cfg.contrastColor)
                b.onPress = { [weak s] in s?.pressed(t) }
                addSubview(b)
                return b
            }
        }
        let l = ButtonRing.layout(selection: sel, screen: bounds, count: buttons.count, button: cfg.buttonSize)
        let animate = s.takeRingNew()
        for (b, f) in zip(buttons, l.frames) {
            b.frame = f
            b.isHidden = false
            b.active = (b.tool == s.tool) || (b.tool == .move && s.moveMode)
            if b.tool == .badge { b.badge = "\(Int(sel.width.rounded()))\n\(Int(sel.height.rounded()))" }
            b.needsDisplay = true
            if animate { b.emerge() }
        }
        sidePanel?.refresh()
    }

    var ringFrames: [(ShotTool, CGRect)] {
        buttons.filter { !$0.isHidden }.map { ($0.tool, $0.frame) }
    }
    var helpShown: Bool { help != nil }
    var modePillFrame: CGRect? { modePill?.frame }

    func showSizeIndicator(_ v: Int) {
        if indicator == nil {
            let i = ShotSizeIndicator()
            addSubview(i)
            indicator = i
        }
        indicator?.show(v, ui: session?.cfg.uiColor ?? .black)
    }

    func showWheel(center: CGPoint, hot: Int?) {
        guard let s = session else { return }
        if wheelView == nil {
            let w = ShotWheelView(colors: s.cfg.userColors)
            addSubview(w)
            wheelView = w
        }
        wheelView?.update(center: center, hot: hot, current: s.color, ui: s.cfg.uiColor)
    }
    func hideWheel() { wheelView?.removeFromSuperview(); wheelView = nil }
    func wheelIndex(at p: CGPoint, center: CGPoint, on: Bool) -> Int? {
        guard on, let s = session else { return nil }
        return ShotWheelView.index(at: p, center: center, count: s.cfg.userColors.count)
    }

    func showLoupe(at p: CGPoint, grab: Bool) {
        guard let s = session, let d = display else { return }
        if loupe == nil {
            let l = ShotLoupe(square: s.cfg.squareMagnifier)
            addSubview(l)
            loupe = l
        }
        loupe?.update(at: p, canvas: d.canvas, bounds: bounds, grab: grab, ui: s.cfg.uiColor)
    }
    func hideLoupe() { loupe?.removeFromSuperview(); loupe = nil }

    func setSidePanel(_ s: ShotSession?) {
        if let s {
            if sidePanel == nil {
                let p = ShotSidePanel(session: s, height: bounds.height)
                addSubview(p)
                sidePanel = p
                p.slideIn()
            }
        } else {
            sidePanel?.removeFromSuperview()
            sidePanel = nil
            window?.makeFirstResponder(self)
        }
        layoutChrome()
    }

    func showShortcuts(_ rows: [(String, String)]) {
        hideShortcuts()
        let c = ShotShortcutsCard(rows: rows, ui: session?.cfg.uiColor ?? .black)
        c.frame.origin = CGPoint(x: ((bounds.width - c.frame.width) / 2).rounded(),
                                 y: ((bounds.height - c.frame.height) / 2).rounded())
        addSubview(c)
        shortcuts = c
    }
    func hideShortcuts() { shortcuts?.removeFromSuperview(); shortcuts = nil }

    func updateCursor(at p: CGPoint) {
        guard let s = session else { return }
        let cur: NSCursor
        if s.grabbing != nil {
            cur = .crosshair
        } else if s.active === display, let sel = s.selection,
                  let h = Self.handlePoints(sel).firstIndex(where: { ShotGeom.dist($0, p) <= max(6, s.cfg.buttonSize * 0.3) + 2 }) {
            cur = [1, 5].contains(h) ? .resizeUpDown : [3, 7].contains(h) ? .resizeLeftRight : .crosshair
        } else if s.moveMode {
            cur = .openHand
        } else if s.active === display, s.tool == nil, let sel = s.selection, sel.contains(p) {
            cur = .openHand
        } else if s.tool == .text {
            cur = .iBeam
        } else {
            cur = .crosshair
        }
        cur.set()
    }
    override func cursorUpdate(with event: NSEvent) {
        updateCursor(at: convert(event.locationInWindow, from: nil))
    }

    private func point(_ e: NSEvent) -> CGPoint { convert(e.locationInWindow, from: nil) }

    override func mouseDown(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        window?.makeKey()
        s.mouseDown(e, d, point(e))
    }
    override func mouseDragged(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.mouseDragged(e, d, point(e))
    }
    override func mouseUp(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.mouseUp(e, d, point(e))
    }
    override func mouseMoved(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.mouseMoved(d, point(e))
        if help == nil || s.mouseDisplay !== d { s.redrawAllChrome() }
    }
    override func scrollWheel(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.scroll(e, d)
    }
    override func magnify(with e: NSEvent) {}
    override func rightMouseDown(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.rightDown(d, point(e))
    }
    override func rightMouseDragged(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.rightDragged(d, point(e))
    }
    override func rightMouseUp(with e: NSEvent) {
        guard let s = session, let d = display else { return }
        s.rightUp(d, point(e))
    }
    override func keyDown(with event: NSEvent) {}
}

extension ShotSession {
    func redrawAllChrome() {
        let showing = displays.contains { $0.view.helpShown }
        let want = cfg.showHelp && selection == nil
        guard want else { return }
        if !showing || displays.contains(where: { $0.view.helpShown && $0 !== mouseDisplay }) {
            for d in displays { d.view.layoutChrome() }
        }
    }
}

final class ShotButton: NSView {
    let tool: ShotTool
    var onPress: (() -> Void)?
    var active = false
    var badge = ""
    private let ui: ShotColor, contrast: ShotColor
    private var hover = false
    private var tracking: NSTrackingArea?

    init(tool: ShotTool, size: CGFloat, ui: ShotColor, contrast: ShotColor) {
        self.tool = tool
        self.ui = ui
        self.contrast = contrast
        super.init(frame: NSRect(x: 0, y: 0, width: size, height: size))
        wantsLayer = true
        layer?.shadowColor = NSColor.black.cgColor
        layer?.shadowOpacity = 0.35
        layer?.shadowRadius = 3
        layer?.shadowOffset = CGSize(width: 0, height: -1)
        toolTip = tool.tooltip
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func layout() {
        super.layout()
        layer?.shadowPath = CGPath(ellipseIn: bounds, transform: nil)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect, .cursorUpdate], owner: self)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }
    override func cursorUpdate(with event: NSEvent) { NSCursor.arrow.set() }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { onPress?() }
    }
    override func rightMouseDown(with event: NSEvent) {}

    func emerge() {
        guard let l = layer else { return }
        let c = CGPoint(x: bounds.midX, y: bounds.midY)
        var t0 = CATransform3DMakeTranslation(c.x, c.y, 0)
        t0 = CATransform3DScale(t0, 0.01, 0.01, 1)
        t0 = CATransform3DTranslate(t0, -c.x, -c.y, 0)
        let a = CABasicAnimation(keyPath: "transform")
        a.fromValue = NSValue(caTransform3D: t0)
        a.toValue = NSValue(caTransform3D: CATransform3DIdentity)
        a.duration = 0.08
        a.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        l.add(a, forKey: "emerge")
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        var fill = hover ? ui.mixed(with: .white, 0.18) : ui
        if active { fill = ui.mixed(with: .black, 0.25) }
        ctx.setFillColor(fill.cgColor)
        ctx.fillEllipse(in: bounds.insetBy(dx: 0.5, dy: 0.5))
        if active {
            ctx.setStrokeColor(contrast.mixed(with: .white, 0.55).cgColor)
            ctx.setLineWidth(2)
            ctx.strokeEllipse(in: bounds.insetBy(dx: 1.5, dy: 1.5))
        }
        let fg: NSColor = ui.isDark ? .white : .black
        if tool == .badge {
            let lines = badge.split(separator: "\n").map(String.init)
            let font = NSFont.systemFont(ofSize: max(8, bounds.height * 0.27), weight: .bold)
            let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
            let lh = font.ascender - font.descender
            var y = (bounds.height - lh * CGFloat(lines.count)) / 2
            for l in lines {
                let w = (l as NSString).size(withAttributes: attrs).width
                (l as NSString).draw(at: CGPoint(x: (bounds.width - w) / 2, y: y), withAttributes: attrs)
                y += lh
            }
            return
        }
        let cfg = NSImage.SymbolConfiguration(pointSize: bounds.height * 0.42, weight: .medium)
            .applying(.init(paletteColors: [fg]))
        guard let img = NSImage(systemSymbolName: tool.symbol, accessibilityDescription: tool.tooltip)?
                .withSymbolConfiguration(cfg) else { return }
        let sz = img.size
        img.draw(in: NSRect(x: (bounds.width - sz.width) / 2, y: (bounds.height - sz.height) / 2,
                            width: sz.width, height: sz.height),
                 from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    }
}

final class ShotHelpCard: NSView {
    private let rows: [(String, String)]
    private let ui: ShotColor
    private let font = NSFont.systemFont(ofSize: 14)
    private let bold = NSFont.systemFont(ofSize: 14, weight: .bold)

    init(rows: [(String, String)], ui: ShotColor) {
        self.rows = rows
        self.ui = ui
        let bold = NSFont.systemFont(ofSize: 14, weight: .bold), font = NSFont.systemFont(ofSize: 14)
        let keyW = rows.map { ($0.0 as NSString).size(withAttributes: [.font: bold]).width }.max() ?? 0
        let actW = rows.map { ($0.1 as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
        let lh = ceil(bold.ascender - bold.descender + 6)
        super.init(frame: NSRect(x: 0, y: 0, width: ceil(keyW + actW + 18 + 48), height: lh * CGFloat(rows.count) + 32))
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.backgroundColor = ui.with(alpha: 0.92).cgColor
        layer?.borderColor = NSColor.white.withAlphaComponent(0.45).cgColor
        layer?.borderWidth = 1
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        let fg: NSColor = ui.isDark ? .white : .black
        let keyW = rows.map { ($0.0 as NSString).size(withAttributes: [.font: bold]).width }.max() ?? 0
        let lh = ceil(bold.ascender - bold.descender + 6)
        var y: CGFloat = 16
        for (k, a) in rows {
            let kw = (k as NSString).size(withAttributes: [.font: bold]).width
            (k as NSString).draw(at: CGPoint(x: 24 + keyW - kw, y: y), withAttributes: [.font: bold, .foregroundColor: fg])
            (a as NSString).draw(at: CGPoint(x: 24 + keyW + 18, y: y), withAttributes: [.font: font, .foregroundColor: fg])
            y += lh
        }
    }
}

final class ShotToolTab: NSView {
    private let ui: ShotColor
    private let action: () -> Void
    private let text = "Tool Settings"
    private let font = NSFont.systemFont(ofSize: 12, weight: .semibold)

    init(ui: ShotColor, action: @escaping () -> Void) {
        self.ui = ui
        self.action = action
        let w = (text as NSString).size(withAttributes: [.font: font]).width
        super.init(frame: NSRect(x: 0, y: 0, width: 22, height: ceil(w) + 24))
        wantsLayer = true
        layer?.backgroundColor = ui.cgColor
        layer?.cornerRadius = 5
        layer?.maskedCorners = [.layerMaxXMinYCorner, .layerMaxXMaxYCorner]
        toolTip = "Open side panel (Space)"
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) { action() }
    override func cursorUpdate(with event: NSEvent) { NSCursor.arrow.set() }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let fg: NSColor = ui.isDark ? .white : .black
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
        let sz = (text as NSString).size(withAttributes: attrs)
        ctx.saveGState()
        ctx.translateBy(x: bounds.midX, y: bounds.midY)
        ctx.rotate(by: .pi / 2)
        (text as NSString).draw(at: CGPoint(x: -sz.width / 2, y: -sz.height / 2), withAttributes: attrs)
        ctx.restoreGState()
    }
}

let shotTextBlue = ShotColor(hex: "#0A84FF")!

final class ShotModePill: NSView {
    private let ui: ShotColor
    private let action: (Bool) -> Void
    private let font = NSFont.systemFont(ofSize: 16, weight: .semibold)
    private let segments: [(symbol: String, title: String)] = [("text.viewfinder", "Copy Text"), ("camera", "Screenshot")]
    private var widths: [CGFloat] = []
    var textMode = false { didSet { if textMode != oldValue { needsDisplay = true } } }

    init(ui: ShotColor, action: @escaping (Bool) -> Void) {
        self.ui = ui
        self.action = action
        super.init(frame: .zero)
        widths = segments.map { ceil(($0.title as NSString).size(withAttributes: [.font: font]).width) + 18 + 8 + 28 }
        frame = NSRect(x: 0, y: 0, width: widths.reduce(12, +), height: 42)
        wantsLayer = true
        layer?.cornerRadius = 21
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.72).cgColor
        layer?.borderColor = NSColor.white.withAlphaComponent(0.25).cgColor
        layer?.borderWidth = 1
        toolTip = "Switch mode (Tab)"
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func rightMouseDown(with event: NSEvent) {}
    override func cursorUpdate(with event: NSEvent) { NSCursor.arrow.set() }
    override func mouseUp(with event: NSEvent) {
        let x = convert(event.locationInWindow, from: nil).x
        action(x >= 4 + (widths.first ?? 0))
    }

    private func segmentRect(_ i: Int) -> CGRect {
        let x = 4 + widths.prefix(i).reduce(0, +)
        return CGRect(x: x, y: 4, width: widths[i], height: bounds.height - 8)
    }

    override func draw(_ dirtyRect: NSRect) {
        for (i, seg) in segments.enumerated() {
            let r = segmentRect(i)
            let on = (i == 0) == textMode
            if on, let ctx = NSGraphicsContext.current?.cgContext {
                let bgColor = (i == 0) ? shotTextBlue.cgColor : ui.cgColor
                ctx.setFillColor(bgColor)
                ctx.addPath(CGPath(roundedRect: r, cornerWidth: r.height / 2, cornerHeight: r.height / 2, transform: nil))
                ctx.fillPath()
            }
            let fg: NSColor = on ? .white : NSColor.white.withAlphaComponent(0.75)
            let cfg = NSImage.SymbolConfiguration(pointSize: 16, weight: .semibold).applying(.init(paletteColors: [fg]))
            let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
            let tw = (seg.title as NSString).size(withAttributes: attrs)
            let content = 18 + 8 + tw.width
            var x = r.minX + (r.width - content) / 2
            if let img = NSImage(systemSymbolName: seg.symbol, accessibilityDescription: nil)?.withSymbolConfiguration(cfg) {
                let s = img.size
                img.draw(in: CGRect(x: x + (18 - s.width) / 2, y: r.midY - s.height / 2, width: s.width, height: s.height),
                         from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            }
            x += 18 + 8
            (seg.title as NSString).draw(at: CGPoint(x: x, y: r.midY - tw.height / 2), withAttributes: attrs)
        }
    }
}

final class ShotRecentButton: NSView {
    private let ui: ShotColor
    private let action: () -> Void
    private let size: CGFloat = 28

    init(ui: ShotColor, action: @escaping () -> Void) {
        self.ui = ui
        self.action = action
        super.init(frame: NSRect(x: 0, y: 0, width: size, height: size))
        wantsLayer = true
        layer?.cornerRadius = size / 2
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.72).cgColor
        layer?.borderColor = NSColor.white.withAlphaComponent(0.25).cgColor
        layer?.borderWidth = 1
        toolTip = "Recent screenshots (⌘R)"
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func rightMouseDown(with event: NSEvent) {}
    override func cursorUpdate(with event: NSEvent) { NSCursor.arrow.set() }
    override func mouseUp(with event: NSEvent) { action() }
    override func draw(_ dirtyRect: NSRect) {
        let cfg = NSImage.SymbolConfiguration(pointSize: 14, weight: .semibold)
        if let base = NSImage(systemSymbolName: "clock.arrow.circlepath", accessibilityDescription: nil)?.withSymbolConfiguration(cfg) {
            let img = NSImage(size: base.size, flipped: false) { r in
                base.draw(in: r)
                NSColor.white.set()
                r.fill(using: .sourceAtop)
                return true
            }
            let s = img.size
            img.draw(in: CGRect(x: (bounds.width - s.width) / 2, y: (bounds.height - s.height) / 2, width: s.width, height: s.height),
                     from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        }
    }
}

final class ShotSizeIndicator: NSView {
    private var value = 0
    private var ui = ShotColor.black
    private var gen = 0

    init() {
        super.init(frame: NSRect(x: 20, y: 20, width: 56, height: 44))
        wantsLayer = true
        layer?.cornerRadius = 8
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func show(_ v: Int, ui: ShotColor) {
        value = v
        self.ui = ui
        layer?.backgroundColor = ui.with(alpha: 0.92).cgColor
        alphaValue = 1
        needsDisplay = true
        gen += 1
        let g = gen
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
            guard let self, g == self.gen else { return }
            NSAnimationContext.runAnimationGroup { $0.duration = 0.3; self.animator().alphaValue = 0 }
        }
    }
    override func draw(_ dirtyRect: NSRect) {
        let s = "\(value)" as NSString
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedDigitSystemFont(ofSize: 22, weight: .bold),
                                                    .foregroundColor: ui.isDark ? NSColor.white : .black]
        let sz = s.size(withAttributes: attrs)
        s.draw(at: CGPoint(x: (bounds.width - sz.width) / 2, y: (bounds.height - sz.height) / 2), withAttributes: attrs)
    }
}

final class ShotWheelView: NSView {
    private let colors: [ShotColor?]
    private var hot: Int?
    private var current = ShotColor.black
    private var ui = ShotColor.black
    static let dot: CGFloat = 26

    static func radius(_ n: Int) -> CGFloat { max(56, CGFloat(n) * (dot + 8) / (2 * .pi)) }

    init(colors: [ShotColor?]) {
        self.colors = colors
        let r = Self.radius(colors.count) + Self.dot
        super.init(frame: NSRect(x: 0, y: 0, width: 2 * r, height: 2 * r))
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    static func index(at p: CGPoint, center c: CGPoint, count n: Int) -> Int? {
        guard n > 0 else { return nil }
        let r = radius(n)
        for i in 0..<n {
            let a = -CGFloat.pi / 2 + 2 * .pi * CGFloat(i) / CGFloat(n)
            let q = CGPoint(x: c.x + cos(a) * r, y: c.y + sin(a) * r)
            if ShotGeom.dist(p, q) <= dot / 2 + 4 { return i }
        }
        return nil
    }

    func update(center: CGPoint, hot: Int?, current: ShotColor, ui: ShotColor) {
        self.hot = hot
        self.current = current
        self.ui = ui
        setFrameOrigin(CGPoint(x: center.x - frame.width / 2, y: center.y - frame.height / 2))
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let c = CGPoint(x: bounds.midX, y: bounds.midY)
        let r = Self.radius(colors.count)
        ctx.setFillColor(NSColor.black.withAlphaComponent(0.35).cgColor)
        ctx.fillEllipse(in: CGRect(x: c.x - r - Self.dot * 0.75, y: c.y - r - Self.dot * 0.75,
                                   width: 2 * (r + Self.dot * 0.75), height: 2 * (r + Self.dot * 0.75)))
        ctx.setFillColor(current.cgColor)
        ctx.fillEllipse(in: CGRect(x: c.x - 16, y: c.y - 16, width: 32, height: 32))
        ctx.setStrokeColor(NSColor.white.cgColor)
        ctx.setLineWidth(2)
        ctx.strokeEllipse(in: CGRect(x: c.x - 16, y: c.y - 16, width: 32, height: 32))
        for (i, col) in colors.enumerated() {
            let a = -CGFloat.pi / 2 + 2 * .pi * CGFloat(i) / CGFloat(colors.count)
            let q = CGPoint(x: c.x + cos(a) * r, y: c.y + sin(a) * r)
            let d = Self.dot * (i == hot ? 1.3 : 1)
            let rect = CGRect(x: q.x - d / 2, y: q.y - d / 2, width: d, height: d)
            if let col {
                ctx.setFillColor(col.cgColor)
                ctx.fillEllipse(in: rect)
            } else {
                for k in 0..<12 {
                    let a0 = CGFloat(k) / 12 * 2 * .pi, a1 = CGFloat(k + 1) / 12 * 2 * .pi
                    ctx.setFillColor(ShotColor(h: Double(k) / 12, s: 1, v: 1).cgColor)
                    ctx.move(to: q)
                    ctx.addArc(center: q, radius: d / 2, startAngle: a0, endAngle: a1, clockwise: false)
                    ctx.fillPath()
                }
            }
            ctx.setStrokeColor((i == hot ? NSColor.white : NSColor.white.withAlphaComponent(0.6)).cgColor)
            ctx.setLineWidth(i == hot ? 2.5 : 1)
            ctx.strokeEllipse(in: rect)
        }
    }
}

final class ShotLoupe: NSView {
    private let square: Bool
    private var image: CGImage?
    private var hex = ""
    private var grab = false
    private var ui = ShotColor.black
    static let side: CGFloat = 132
    static let px = 15

    init(square: Bool) {
        self.square = square
        super.init(frame: NSRect(x: 0, y: 0, width: Self.side, height: Self.side + 22))
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func update(at p: CGPoint, canvas: ShotCanvas, bounds b: CGRect, grab: Bool, ui: ShotColor) {
        self.grab = grab
        self.ui = ui
        let cx = Int(p.x * canvas.scale), cy = Int(p.y * canvas.scale)
        let half = Self.px / 2
        image = canvas.base.cropping(to: CGRect(x: cx - half, y: cy - half, width: Self.px, height: Self.px))
        hex = canvas.color(at: p)?.hex.uppercased() ?? ""
        var o = CGPoint(x: p.x + 24, y: p.y + 24)
        if o.x + frame.width > b.maxX { o.x = p.x - 24 - frame.width }
        if o.y + frame.height > b.maxY { o.y = p.y - 24 - frame.height }
        setFrameOrigin(o)
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let r = CGRect(x: 0, y: 0, width: Self.side, height: Self.side)
        ctx.saveGState()
        let path = square ? CGPath(rect: r, transform: nil) : CGPath(ellipseIn: r, transform: nil)
        ctx.addPath(path)
        ctx.clip()
        ctx.setFillColor(NSColor.black.cgColor)
        ctx.fill(r)
        if let image { ShotRenderer.drawImage(image, in: r, ctx, smooth: false) }
        let cell = Self.side / CGFloat(Self.px)
        ctx.setStrokeColor(NSColor.white.cgColor)
        ctx.setLineWidth(1)
        ctx.stroke(CGRect(x: cell * CGFloat(Self.px / 2), y: cell * CGFloat(Self.px / 2), width: cell, height: cell))
        ctx.restoreGState()
        ctx.addPath(path)
        ctx.setStrokeColor(ui.cgColor)
        ctx.setLineWidth(2)
        ctx.strokePath()
        if grab {
            let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: 12, weight: .semibold),
                                                        .foregroundColor: NSColor.white]
            let s = hex as NSString
            let sz = s.size(withAttributes: attrs)
            let box = CGRect(x: (Self.side - sz.width) / 2 - 6, y: Self.side + 2, width: sz.width + 12, height: 18)
            ctx.setFillColor(ui.with(alpha: 0.95).cgColor)
            ctx.addPath(CGPath(roundedRect: box, cornerWidth: 4, cornerHeight: 4, transform: nil))
            ctx.fillPath()
            s.draw(at: CGPoint(x: box.minX + 6, y: box.minY + 1), withAttributes: attrs)
        }
    }
}

final class ShotTextField: NSTextView {
    var object: ShotObject
    var onChange: (() -> Void)?
    private let store: NSTextStorage

    init(object: ShotObject) {
        self.object = object
        let tc = NSTextContainer(size: NSSize(width: 1e6, height: 1e6))
        tc.widthTracksTextView = false
        tc.lineFragmentPadding = 0
        let lm = NSLayoutManager()
        lm.addTextContainer(tc)
        let st = NSTextStorage(string: object.text)
        st.addLayoutManager(lm)
        store = st
        super.init(frame: NSRect(origin: object.start, size: CGSize(width: 40, height: 30)), textContainer: tc)
        drawsBackground = false
        isRichText = false
        allowsUndo = true
        isHorizontallyResizable = true
        isVerticallyResizable = true
        textContainerInset = NSSize(width: ShotText.padding, height: ShotText.padding)
        isAutomaticQuoteSubstitutionEnabled = false
        isAutomaticDashSubstitutionEnabled = false
        isAutomaticSpellingCorrectionEnabled = false
        isContinuousSpellCheckingEnabled = false
        wantsLayer = true
        layer?.borderWidth = 1
        layer?.borderColor = NSColor.white.withAlphaComponent(0.7).cgColor
        applyStyle(size: nil)
    }
    required init?(coder: NSCoder) { fatalError() }
    override init(frame frameRect: NSRect, textContainer container: NSTextContainer?) {
        object = ShotObject(tool: .text, points: [.zero], color: .black, size: 8)
        store = container?.layoutManager?.textStorage ?? NSTextStorage()
        super.init(frame: frameRect, textContainer: container)
    }

    func applyStyle(size: Int?) {
        if let size { object.size = size }
        let f = ShotText.font(object.style, size: object.fontSize) as NSFont
        font = f
        textColor = NSColor(cgColor: object.color.cgColor)
        insertionPointColor = textColor ?? .white
        alignment = [.left, .center, .right][max(0, min(2, object.style.align))]
        typingAttributes = [.font: f, .foregroundColor: textColor ?? .white,
                            .underlineStyle: object.style.underline ? NSUnderlineStyle.single.rawValue : 0,
                            .strikethroughStyle: object.style.strike ? NSUnderlineStyle.single.rawValue : 0]
        if let ts = textStorage {
            ts.setAttributes(typingAttributes, range: NSRange(location: 0, length: ts.length))
        }
        place()
    }

    func place() {
        var o = object
        o.text = string
        let sz = ShotText.boxSize(o)
        frame = NSRect(origin: object.start, size: CGSize(width: max(sz.width, 24), height: sz.height))
    }

    override func didChangeText() {
        super.didChangeText()
        place()
        onChange?()
    }
}

final class ShotShortcutsCard: NSView {
    private let rows: [(String, String)]
    private let ui: ShotColor
    private let font = NSFont.systemFont(ofSize: 13)
    private let bold = NSFont.systemFont(ofSize: 13, weight: .semibold)

    init(rows: [(String, String)], ui: ShotColor) {
        self.rows = rows.isEmpty ? [("Esc", "close this card")] : rows
        self.ui = ui
        let bold = NSFont.systemFont(ofSize: 13, weight: .semibold), font = NSFont.systemFont(ofSize: 13)
        let kw = self.rows.map { ($0.0 as NSString).size(withAttributes: [.font: bold]).width }.max() ?? 0
        let aw = self.rows.map { ($0.1 as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
        let lh: CGFloat = 20
        super.init(frame: NSRect(x: 0, y: 0, width: ceil(kw + aw + 64), height: lh * CGFloat(self.rows.count) + 52))
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.backgroundColor = NSColor(white: 0.08, alpha: 0.94).cgColor
        layer?.borderColor = ui.cgColor
        layer?.borderWidth = 1.5
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        let kw = rows.map { ($0.0 as NSString).size(withAttributes: [.font: bold]).width }.max() ?? 0
        ("Keyboard Shortcuts" as NSString).draw(at: CGPoint(x: 24, y: 14), withAttributes: [.font: NSFont.systemFont(ofSize: 14, weight: .bold), .foregroundColor: NSColor.white])
        var y: CGFloat = 40
        for (k, a) in rows {
            (k as NSString).draw(at: CGPoint(x: 24, y: y), withAttributes: [.font: bold, .foregroundColor: NSColor.white])
            (a as NSString).draw(at: CGPoint(x: 24 + kw + 16, y: y), withAttributes: [.font: font, .foregroundColor: NSColor(white: 0.8, alpha: 1)])
            y += 20
        }
    }
}

final class ShotSidePanel: NSView, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    private weak var session: ShotSession?
    static let width: CGFloat = 250
    private let sizeStepper = NSStepper()
    private let sizeValue = NSTextField(labelWithString: "")
    private let sizeSlider = NSSlider(value: 3, minValue: 1, maxValue: 100, target: nil, action: nil)
    private let swatch = NSView()
    private let colorName = NSTextField(labelWithString: "")
    private let hsv = ShotHSVWheel()
    private let brightness = NSSlider(value: 1, minValue: 0, maxValue: 1, target: nil, action: nil)
    private let hexField = NSTextField(string: "")
    private let gridCheck = NSButton(checkboxWithTitle: "Display grid", target: nil, action: nil)
    private let gridStepper = NSStepper()
    private let gridValue = NSTextField(labelWithString: "")
    private let fontPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private var styleButtons: [NSButton] = []
    private let align = NSSegmentedControl(labels: ["Left", "Center", "Right"], trackingMode: .selectOne, target: nil, action: nil)
    private let textBox = ShotFlippedView()
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private var targets: [ClosureTarget] = []
    private var updating = false

    init(session: ShotSession, height: CGFloat) {
        self.session = session
        super.init(frame: NSRect(x: 0, y: 0, width: Self.width, height: height))
        appearance = NSAppearance(named: .darkAqua)
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.06, alpha: 0.86).cgColor
        build()
        refresh()
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func rightMouseDown(with event: NSEvent) {}
    override func scrollWheel(with event: NSEvent) {}
    override func cursorUpdate(with event: NSEvent) { NSCursor.arrow.set() }

    func slideIn() {
        let f = frame
        setFrameOrigin(CGPoint(x: -f.width, y: f.minY))
        NSAnimationContext.runAnimationGroup { $0.duration = 0.15; self.animator().setFrameOrigin(f.origin) }
    }

    private func act(_ c: NSControl, _ f: @escaping () -> Void) {
        let t = ClosureTarget(action: f)
        targets.append(t)
        c.target = t
        c.action = #selector(ClosureTarget.run)
    }
    private func label(_ s: String, _ y: CGFloat) -> CGFloat {
        let l = NSTextField(labelWithString: s)
        l.font = .systemFont(ofSize: 12, weight: .semibold)
        l.textColor = .white
        l.frame = NSRect(x: 14, y: y, width: Self.width - 28, height: 16)
        addSubview(l)
        return y + 20
    }

    private func build() {
        var y: CGFloat = 16
        y = label("Active tool size:", y)
        sizeValue.frame = NSRect(x: 14, y: y + 2, width: 40, height: 18)
        sizeValue.textColor = .white
        sizeStepper.frame = NSRect(x: 54, y: y, width: 20, height: 24)
        sizeStepper.minValue = 0; sizeStepper.maxValue = 100; sizeStepper.increment = 1
        sizeSlider.frame = NSRect(x: 82, y: y + 2, width: Self.width - 96, height: 20)
        act(sizeStepper) { [weak self] in self?.session?.setSize(Int(self?.sizeStepper.intValue ?? 1)) }
        act(sizeSlider) { [weak self] in self?.session?.setSize(Int(self?.sizeSlider.intValue ?? 1)) }
        [sizeValue, sizeStepper, sizeSlider].forEach(addSubview)
        y += 34

        y = label("Active Color:", y)
        swatch.wantsLayer = true
        swatch.layer?.cornerRadius = 4
        swatch.layer?.borderColor = NSColor.white.withAlphaComponent(0.6).cgColor
        swatch.layer?.borderWidth = 1
        swatch.frame = NSRect(x: 14, y: y, width: 28, height: 22)
        colorName.frame = NSRect(x: 50, y: y + 3, width: 90, height: 18)
        colorName.textColor = .white
        colorName.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        let grab = NSButton(title: "Grab Color", target: nil, action: nil)
        grab.bezelStyle = .rounded
        grab.frame = NSRect(x: Self.width - 110, y: y - 2, width: 96, height: 26)
        act(grab) { [weak self] in self?.session?.startGrab() }
        [swatch, colorName, grab].forEach(addSubview)
        y += 32

        hsv.frame = NSRect(x: (Self.width - 170) / 2, y: y, width: 170, height: 170)
        hsv.onPick = { [weak self] h, s in
            guard let self else { return }
            self.session?.setColor(ShotColor(h: h, s: s, v: self.brightness.doubleValue))
        }
        addSubview(hsv)
        y += 176
        brightness.frame = NSRect(x: 14, y: y, width: Self.width - 28, height: 20)
        act(brightness) { [weak self] in
            guard let self, let c = self.session?.currentColor else { return }
            let v = c.hsv
            self.session?.setColor(ShotColor(h: v.h, s: v.s, v: self.brightness.doubleValue))
        }
        addSubview(brightness)
        y += 28
        hexField.frame = NSRect(x: 14, y: y, width: 110, height: 22)
        hexField.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        hexField.placeholderString = "#RRGGBB"
        hexField.delegate = self
        act(hexField) { [weak self] in self?.applyHex() }
        addSubview(hexField)
        y += 34

        gridCheck.frame = NSRect(x: 14, y: y, width: 120, height: 20)
        act(gridCheck) { [weak self] in
            guard let self, let s = self.session else { return }
            s.state.grid = self.gridCheck.state == .on
            s.state.save(s.statePath)
            s.redrawAll()
        }
        gridStepper.frame = NSRect(x: 150, y: y - 2, width: 20, height: 24)
        gridStepper.minValue = 5; gridStepper.maxValue = 50; gridStepper.increment = 5
        gridValue.frame = NSRect(x: 176, y: y + 1, width: 40, height: 18)
        gridValue.textColor = .white
        act(gridStepper) { [weak self] in
            guard let self, let s = self.session else { return }
            s.state.gridSize = Int(self.gridStepper.intValue)
            s.state.save(s.statePath)
            s.redrawAll()
        }
        [gridCheck, gridStepper, gridValue].forEach(addSubview)
        y += 32

        textBox.frame = NSRect(x: 0, y: y, width: Self.width, height: 92)
        let tl = NSTextField(labelWithString: "Text:")
        tl.font = .systemFont(ofSize: 12, weight: .semibold)
        tl.textColor = .white
        tl.frame = NSRect(x: 14, y: 0, width: 60, height: 16)
        fontPopup.frame = NSRect(x: 14, y: 20, width: Self.width - 28, height: 24)
        fontPopup.addItem(withTitle: "System")
        fontPopup.addItems(withTitles: NSFontManager.shared.availableFontFamilies)
        act(fontPopup) { [weak self] in
            guard let self else { return }
            let i = self.fontPopup.indexOfSelectedItem
            let fam = i <= 0 ? "" : self.fontPopup.titleOfSelectedItem ?? ""
            self.session?.setTextStyle { $0.family = fam }
        }
        let styles: [(String, WritableKeyPath<ShotTextStyle, Bool>)] = [("B", \.bold), ("I", \.italic), ("U", \.underline), ("S", \.strike)]
        for (i, (t, kp)) in styles.enumerated() {
            let b = NSButton(title: t, target: nil, action: nil)
            b.setButtonType(.pushOnPushOff)
            b.bezelStyle = .rounded
            b.frame = NSRect(x: 14 + CGFloat(i) * 36, y: 52, width: 34, height: 26)
            act(b) { [weak self, weak b] in self?.session?.setTextStyle { $0[keyPath: kp] = b?.state == .on } }
            styleButtons.append(b)
            textBox.addSubview(b)
        }
        align.frame = NSRect(x: 162, y: 54, width: Self.width - 176, height: 24)
        align.setLabel("", forSegment: 0); align.setLabel("", forSegment: 1); align.setLabel("", forSegment: 2)
        for (i, n) in ["text.alignleft", "text.aligncenter", "text.alignright"].enumerated() {
            align.setImage(NSImage(systemSymbolName: n, accessibilityDescription: nil), forSegment: i)
            align.setWidth((Self.width - 182) / 3, forSegment: i)
        }
        act(align) { [weak self] in
            guard let self else { return }
            let v = self.align.selectedSegment
            self.session?.setTextStyle { $0.align = v }
        }
        [tl, fontPopup, align].forEach(textBox.addSubview)
        addSubview(textBox)
        y += 100

        y = label("Layers", y)
        let col = NSTableColumn(identifier: .init("layer"))
        col.width = Self.width - 30
        table.addTableColumn(col)
        table.headerView = nil
        table.dataSource = self
        table.delegate = self
        table.backgroundColor = .clear
        table.rowHeight = 20
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        let listH = max(80, bounds.height - y - 50)
        scroll.frame = NSRect(x: 14, y: y, width: Self.width - 28, height: listH)
        addSubview(scroll)
        y += listH + 8
        let buttons: [(String, String, () -> Void)] = [
            ("trash", "Delete", { [weak self] in self?.deleteLayer() }),
            ("arrow.up", "Move up", { [weak self] in self?.moveLayer(1) }),
            ("arrow.down", "Move down", { [weak self] in self?.moveLayer(-1) }),
        ]
        for (i, (sym, tip, f)) in buttons.enumerated() {
            let b = NSButton(image: NSImage(systemSymbolName: sym, accessibilityDescription: tip) ?? NSImage(), target: nil, action: nil)
            b.bezelStyle = .rounded
            b.toolTip = tip
            b.frame = NSRect(x: 14 + CGFloat(i) * 44, y: y, width: 40, height: 26)
            act(b, f)
            addSubview(b)
        }
    }

    func refresh() {
        guard let s = session else { return }
        updating = true
        defer { updating = false }
        let selObj = s.doc.selected.flatMap { s.doc.objects.indices.contains($0) ? s.doc.objects[$0] : nil }
        let sz = selObj?.size ?? s.activeSize
        sizeValue.stringValue = sz.map(String.init) ?? "–"
        sizeStepper.integerValue = sz ?? 0
        sizeSlider.integerValue = sz ?? 0
        sizeStepper.isEnabled = sz != nil
        sizeSlider.isEnabled = sz != nil
        let c = s.currentColor
        swatch.layer?.backgroundColor = c.cgColor
        colorName.stringValue = c.hex.uppercased()
        let v = c.hsv
        hsv.marker = (v.h, v.s)
        hsv.value = v.v
        brightness.doubleValue = v.v
        if window?.firstResponder !== hexField.currentEditor() { hexField.stringValue = c.hex.uppercased() }
        gridCheck.state = s.state.grid ? .on : .off
        gridStepper.integerValue = s.state.gridSize
        gridValue.stringValue = "\(s.state.gridSize)"
        gridStepper.isEnabled = s.state.grid
        let st = selObj?.tool == .text ? selObj!.style : s.state.style
        textBox.isHidden = !(s.tool == .text || selObj?.tool == .text || s.editing != nil)
        fontPopup.selectItem(withTitle: st.family.isEmpty ? "System" : st.family)
        for (b, on) in zip(styleButtons, [st.bold, st.italic, st.underline, st.strike]) { b.state = on ? .on : .off }
        align.selectedSegment = st.align
        table.reloadData()
        if let i = s.doc.selected {
            table.selectRowIndexes([s.doc.objects.count - 1 - i], byExtendingSelection: false)
        } else {
            table.deselectAll(nil)
        }
    }

    private func applyHex() {
        guard let s = session else { return }
        if let c = ShotColor(hex: hexField.stringValue) { s.setColor(c) }
        else { hexField.stringValue = s.currentColor.hex.uppercased() }
    }
    func leaveField() {
        if let s = session, ShotColor(hex: hexField.stringValue) == nil {
            hexField.stringValue = s.currentColor.hex.uppercased()
        }
    }

    private func deleteLayer() {
        guard let s = session, let i = s.doc.selected else { return }
        s.doc.remove(at: i)
        s.redrawAll()
    }
    private func moveLayer(_ d: Int) {
        guard let s = session, let i = s.doc.selected else { return }
        let to = i + d
        guard s.doc.objects.indices.contains(to) else { return }
        s.doc.reorder(from: i, to: to)
        s.redrawAll()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { session?.doc.objects.count ?? 0 }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let s = session else { return nil }
        let i = s.doc.objects.count - 1 - row
        let o = s.doc.objects[i]
        var title = o.tool.rawValue.capitalized
        if o.tool == .text { title = "Text: " + o.text.replacingOccurrences(of: "\n", with: " ") }
        if o.tool == .counter { title = "Counter \(o.number)" }
        let f = NSTextField(labelWithString: "\(i + 1). \(title)")
        f.textColor = .white
        f.lineBreakMode = .byTruncatingTail
        return f
    }
    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !updating, let s = session else { return }
        let r = table.selectedRow
        s.doc.selected = r < 0 ? nil : s.doc.objects.count - 1 - r
        s.redrawAll()
    }
}

final class ShotFlippedView: NSView {
    override var isFlipped: Bool { true }
}

final class ShotHSVWheel: NSView {
    var onPick: ((Double, Double) -> Void)?
    var marker: (Double, Double) = (0, 0) { didSet { needsDisplay = true } }
    var value: Double = 1 { didSet { if abs(value - oldValue) > 0.01 { disc = nil; needsDisplay = true } } }
    private var disc: CGImage?
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    private func makeDisc(_ side: Int) -> CGImage? {
        guard side > 0 else { return nil }
        var buf = [UInt8](repeating: 0, count: side * side * 4)
        let r = Double(side) / 2
        for y in 0..<side {
            for x in 0..<side {
                let dx = Double(x) + 0.5 - r, dy = r - (Double(y) + 0.5)
                let d = sqrt(dx * dx + dy * dy) / r
                guard d <= 1 else { continue }
                var h = atan2(dy, dx) / (2 * .pi)
                if h < 0 { h += 1 }
                let c = ShotColor(h: h, s: d, v: value)
                let i = (y * side + x) * 4
                buf[i] = UInt8(c.r * 255); buf[i + 1] = UInt8(c.g * 255); buf[i + 2] = UInt8(c.b * 255); buf[i + 3] = 255
            }
        }
        return ShotPixels(width: side, height: side, data: buf).image
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        if disc == nil { disc = makeDisc(Int(bounds.width)) }
        if let disc { ShotRenderer.drawImage(disc, in: bounds, ctx) }
        let r = bounds.width / 2
        let a = marker.0 * 2 * .pi, d = marker.1 * Double(r)
        let p = CGPoint(x: bounds.midX + CGFloat(cos(a) * d), y: bounds.midY - CGFloat(sin(a) * d))
        ctx.setStrokeColor(NSColor.white.cgColor)
        ctx.setLineWidth(2)
        ctx.strokeEllipse(in: CGRect(x: p.x - 5, y: p.y - 5, width: 10, height: 10))
    }
    private func pick(_ e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let r = Double(bounds.width / 2)
        let dx = Double(p.x - bounds.midX), dy = -Double(p.y - bounds.midY)
        var h = atan2(dy, dx) / (2 * .pi)
        if h < 0 { h += 1 }
        onPick?(h, min(1, sqrt(dx * dx + dy * dy) / r))
    }
    override func mouseDown(with event: NSEvent) { pick(event) }
    override func mouseDragged(with event: NSEvent) { pick(event) }
}

final class ShotSaveCard: NSView, NSTextFieldDelegate {
    let field = NSTextField(string: "")
    private let onSave: (String) -> Void
    private var target: ClosureTarget?

    init(path: String, ui: ShotColor, onSave: @escaping (String) -> Void) {
        self.onSave = onSave
        super.init(frame: NSRect(x: 0, y: 0, width: 520, height: 96))
        appearance = NSAppearance(named: .darkAqua)
        wantsLayer = true
        layer?.cornerRadius = 8
        layer?.backgroundColor = NSColor(white: 0.08, alpha: 0.95).cgColor
        layer?.borderColor = ui.cgColor
        layer?.borderWidth = 1.5
        let title = NSTextField(labelWithString: "Save screenshot as")
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        title.textColor = .white
        title.frame = NSRect(x: 16, y: 12, width: 300, height: 18)
        field.stringValue = path
        field.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        field.frame = NSRect(x: 16, y: 36, width: 488, height: 24)
        field.delegate = self
        field.cell?.sendsActionOnEndEditing = false
        let t = ClosureTarget { [weak self] in
            guard let self else { return }
            self.onSave(self.field.stringValue)
        }
        target = t
        field.target = t
        field.action = #selector(ClosureTarget.run)
        let hint = NSTextField(labelWithString: "Return saves (.png / .jpg) · Esc goes back")
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = NSColor(white: 0.7, alpha: 1)
        hint.frame = NSRect(x: 16, y: 68, width: 488, height: 16)
        [title, field, hint].forEach(addSubview)
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {}
    override func rightMouseDown(with event: NSEvent) {}
    override func cursorUpdate(with event: NSEvent) { NSCursor.arrow.set() }

    func focus() {
        window?.makeFirstResponder(field)
        guard let ed = field.currentEditor() else { return }
        let ns = field.stringValue as NSString
        let name = ns.lastPathComponent as NSString
        let start = ns.length - name.length
        ed.selectedRange = NSRange(location: start, length: (name.deletingPathExtension as NSString).length)
    }
}
