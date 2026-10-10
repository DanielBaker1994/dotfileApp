import AppKit

enum SlotView: String {
    case notes, files, jira, detail, releases, config, output, confluence, ai, compare, compareText
    var isJira: Bool { [.jira, .detail, .releases, .config].contains(self) }
    var isCompare: Bool { self == .compare || self == .compareText }
    var isSub: Bool { [.detail, .releases, .config, .output, .compareText].contains(self) }
}

protocol SlotMember: AnyObject {
    var slotWindow: NSWindow { get }
    var slotShown: Bool { get }
    var slotBaseFrame: NSRect { get }
    func slotPark(stopVoice: Bool)
    func slotShow(frame: NSRect?)
    func slotAttach(to host: SlotHostWindow)
    func slotDetach()
}

extension PopupWindow: SlotMember {
    var slotWindow: NSWindow { nativeWindow }
    var slotShown: Bool { isShown }
    var slotBaseFrame: NSRect { baseFrame }
    func slotPark(stopVoice: Bool) { park(stopVoice: stopVoice) }
    func slotShow(frame: NSRect?) { unpark(frame: frame) }
}

final class SlotHostWindow: PopupPlainWindow {
    private(set) weak var guest: AnyObject?
    var onEmpty: (() -> Void)?
    var onCloseRequest: (() -> Void)?

    init() {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 900, height: 600),
                   styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                   backing: .buffered, defer: false)
        titlebarAppearsTransparent = true
        titleVisibility = .hidden
        for b: NSWindow.ButtonType in [.closeButton, .miniaturizeButton, .zoomButton] {
            standardWindowButton(b)?.isHidden = true
        }
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isReleasedWhenClosed = false
        animationBehavior = .none
        acceptsMouseMovedEvents = true
        contentView = NSView()
    }

    override func performClose(_ sender: Any?) { onCloseRequest?() }
    override func close() { onCloseRequest?() }

    func take(_ g: AnyObject, chromeOf w: NSWindow) {
        guest = g
        if let c = w as? CardNSWindow {
            cornerRadius = c.cornerRadius
            headerClickBand = c.headerBand
            onHeaderClick = { [weak self, weak c] p in
                c?.onHeaderClick?(NSPoint(x: p.x, y: (self?.frame.height ?? 0) - p.y))
            }
            onEscape = nil
            selectionAttributes = nil
            clickFocusesField = false
        } else if let b = w as? PopupBaseWindow {
            headerClickBand = b.headerClickBand
            onHeaderClick = b.onHeaderClick
            onEscape = b.onEscape
            selectionAttributes = b.selectionAttributes
            clickFocusesField = true
            if let pw = b as? PopupPlainWindow { cornerRadius = pw.cornerRadius }
        }
        title = w.title
        minSize = w.minSize
        appearance = w.appearance
        hasShadow = w.hasShadow
        collectionBehavior = w.collectionBehavior
        let mine = standardWindowButton(.closeButton), theirs = w.standardWindowButton(.closeButton)
        mine?.isHidden = theirs?.isHidden ?? true
        mine?.target = theirs?.target
        mine?.action = theirs?.action
        invalidateShadow()
    }

    func give(chromeTo w: NSWindow) {
        if let b = w as? PopupBaseWindow, !(w is CardNSWindow) {
            b.headerClickBand = headerClickBand
            b.selectionAttributes = selectionAttributes
            (b as? PopupPlainWindow)?.cornerRadius = cornerRadius
        }
        w.appearance = appearance
    }

    func guestLeft(_ g: AnyObject) {
        guard guest === g else { return }
        guest = nil
        DispatchQueue.main.async { [weak self] in
            guard let self, self.guest == nil, self.isVisible else { return }
            self.onEmpty?()
        }
    }

    static func moveContent(from: NSWindow, to: NSWindow) {
        guard let root = from.contentView else { return }
        var target: NSResponder? = from.firstResponder
        var caret: NSRange?
        if let tv = target as? NSTextView, tv.isFieldEditor, let field = tv.delegate as? NSTextField {
            caret = tv.selectedRange
            target = field
        }
        from.contentView = NSView()
        to.contentView = root
        if let v = target as? NSView, v.isDescendant(of: root), to.makeFirstResponder(v),
           let caret, let ed = (v as? NSTextField)?.currentEditor() {
            ed.selectedRange = caret
        }
    }
}

final class SharedWindow {
    static let navNotes = 60, navJira = 61, navHome = 62, navBack = 63, navFiles = 64, navConfluence = 65,
               navAI = 66, navCompare = 67
    static let navIDs: Set<Int> = [navNotes, navJira, navHome, navBack, navFiles, navConfluence, navAI, navCompare]

    private unowned let controller: SwitcherController
    private(set) var current: SlotView?
    private var last: SlotView = .notes
    private(set) var previousNav: Int?
    private var prefixArmedAt: Date?
    private var prefixHeldKey: UInt16?
    private var lastJira: SlotView = .jira
    private var stack: [SlotView] = []
    private var returnWID: String?
    private var returnPID: pid_t?
    private var summoned = false
    private var swappedAt: Date?
    private var focusLossGen = 0
    var targetScreen: Int?
    var aerospaceCacheCleared = false

    private(set) lazy var host: SlotHostWindow = {
        let h = SlotHostWindow()
        h.onEmpty = { [weak self] in self?.hide("its view went away", restoreFocus: false) }
        h.onCloseRequest = { [weak self] in self?.hide("close") }
        return h
    }()

    init(controller: SwitcherController) {
        self.controller = controller
        let nc = NotificationCenter.default
        nc.addObserver(forName: NSWindow.didResignKeyNotification, object: nil, queue: .main) { [weak self] n in
            guard let self, let w = n.object as? NSWindow, let cur = self.current,
                  let m = self.controller.slotMember(cur), m.slotWindow === w else { return }
            self.focusLossGen += 1
            let gen = self.focusLossGen
            DispatchQueue.main.asyncAfter(deadline: .now() + settings.focusLossDelay) { [weak self] in
                guard let self, gen == self.focusLossGen else { return }
                self.checkFocusLoss(cur, w)
            }
        }
        nc.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) { [weak self] _ in
            self?.focusLossGen += 1
        }
        for name in [NSWindow.didMoveNotification, NSWindow.didEndLiveResizeNotification] {
            nc.addObserver(forName: name, object: nil, queue: .main) { [weak self] n in
                guard let w = n.object as? NSWindow else { return }
                DispatchQueue.main.async {
                    guard let self, let cur = self.current, let m = self.controller.slotMember(cur),
                          m.slotShown, m.slotWindow === w else { return }
                    self.frame = m.slotBaseFrame
                }
            }
        }
    }

    private func checkFocusLoss(_ v: SlotView, _ w: NSWindow) {
        guard current == v, let m = controller.slotMember(v), m.slotShown, m.slotWindow === w,
              focusLeft(w) else { return }
        let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"
        if let t = swappedAt, Date().timeIntervalSince(t) < 1.0 {
            swappedAt = nil
            controller.log("shared window: focus stolen by \(front) right after a view swap — refocused \(v.rawValue)")
            m.slotShow(frame: nil)
            return
        }
        guard settings.hideOnFocusLoss else { return }
        if let p = m as? PopupWindow, p.isShowingMenu { return }
        hide("focus loss → \(front)", restoreFocus: false)
    }

    private static let frameKey = "sharedWindowFrame"
    var frame: NSRect {
        get {
            if let s = UserDefaults.standard.string(forKey: Self.frameKey) {
                let r = NSRectFromString(s)
                if r.width > 200, r.height > 150, let from = Self.screen(of: r) {
                    return Self.place(r, from: from, to: targetNSScreen ?? from)
                }
            }
            let mouse = NSEvent.mouseLocation
            let vis = (targetNSScreen ?? NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main)?.gapFrame
                ?? NSRect(x: 0, y: 0, width: 1400, height: 900)
            let w = min(settings.sharedWidth, vis.width - 40), h = min(settings.sharedHeight, vis.height - 40)
            return NSRect(x: vis.midX - w / 2, y: vis.midY - h / 2, width: w, height: h)
        }
        set {
            guard Self.screen(of: newValue) != nil else { return }
            UserDefaults.standard.set(NSStringFromRect(newValue), forKey: Self.frameKey)
        }
    }

    private var targetNSScreen: NSScreen? {
        guard let i = targetScreen, NSScreen.screens.indices.contains(i - 1) else { return nil }
        return NSScreen.screens[i - 1]
    }

    static func screen(of r: NSRect) -> NSScreen? {
        let area = r.width * r.height
        guard area > 0 else { return nil }
        let best = NSScreen.screens.max { a, b in
            let ia = a.frame.intersection(r), ib = b.frame.intersection(r)
            return ia.width * ia.height < ib.width * ib.height
        }
        guard let best else { return nil }
        let i = best.frame.intersection(r)
        return i.width * i.height >= area / 2 ? best : nil
    }

    static func place(_ r: NSRect, from: NSScreen, to: NSScreen) -> NSRect {
        let fv = from.gapFrame, tv = to.gapFrame
        let w = min(r.width, tv.width), h = min(r.height, tv.height)
        var x = r.minX, y = r.minY
        if from != to {
            x = tv.minX + (r.midX - fv.minX) / fv.width * tv.width - w / 2
            y = tv.minY + (r.midY - fv.minY) / fv.height * tv.height - h / 2
        }
        x = min(max(x, tv.minX), tv.maxX - w)
        y = min(max(y, tv.minY), tv.maxY - h)
        return NSRect(x: x, y: y, width: w, height: h)
    }

    func currentFrame() -> NSRect {
        shownMember?.slotBaseFrame ?? frame
    }

    var isVisible: Bool {
        shownMember != nil
    }

    private var shownMember: SlotMember? {
        current.flatMap { controller.slotMember($0) }.flatMap { $0.slotShown ? $0 : nil }
    }

    private func hideOrFocus(_ m: SlotMember, userInIt: Bool?) {
        let inIt = userInIt ?? (m.slotWindow.isKeyWindow && NSApp.isActive
            && NSWorkspace.shared.frontmostApplication?.processIdentifier == getpid())
        if inIt { hide("hotkey pressed while in it") } else { m.slotShow(frame: nil) }
    }

    func hotkey(_ v: SlotView, userInIt: Bool? = nil) {
        if let cur = current, let m = shownMember, v == .jira ? cur.isJira : cur == v {
            hideOrFocus(m, userInIt: userInIt)
            return
        }
        if v == .jira, lastJira != .jira, controller.slotMember(lastJira) != nil {
            present(lastJira)
        } else {
            open(v)
        }
    }

    func toggle(userInIt: Bool? = nil) {
        if let m = shownMember {
            hideOrFocus(m, userInIt: userInIt)
            return
        }
        if controller.slotMember(last) != nil {
            present(last)
            return
        }
        let top: SlotView = last.isJira ? .jira : last
        if top == .files { controller.slotShowFiles() } else { open(top) }
        if !isVisible && top != .files { controller.slotShowFiles() }
    }

    func open(_ v: SlotView) {
        if v == .jira && current?.isJira == true { stack = [] }
        if !v.isJira { stack.removeAll { $0 == v } }
        guard controller.ensureSlotMember(v, frame: currentFrame()) else { return }
        present(v)
    }

    func push(_ v: SlotView) {
        if let cur = current, isVisible {
            if cur != v { stack.append(cur) }
        } else {
            stack = v.isJira ? [.jira] : v == .compareText ? [.compare] : []
        }
        stack.removeAll { $0 == v }
        present(v)
    }

    func back(esc: Bool = false) {
        while let prev = stack.popLast() {
            if controller.slotMember(prev) != nil || prev == .jira || prev == .compare {
                open(prev)
                return
            }
        }
        if let cur = current, cur.isJira, cur != .jira {
            open(.jira)
        } else if current == .compareText {
            open(.compare)
        } else if let cur = current {
            if esc { escapeAtTop(cur) } else { hide("back from the first view") }
        }
    }

    func home() {
        stack = []
        open(.jira)
    }

    func present(_ v: SlotView) {
        guard let m = controller.slotMember(v) else { return }
        if let cur = current, let a = Self.navOn(cur), let b = Self.navOn(v), a != b { previousNav = a }
        if !summoned {
            summoned = true
            returnWID = controller.savedWID
            returnPID = controller.savedPID
        }
        var f = currentFrame()
        var outgoing: SlotMember?
        if let old = shownMember, old !== m {
            frame = old.slotBaseFrame
            outgoing = old
        }
        let hostUp = host.isVisible
        if let sheet = host.attachedSheet, outgoing != nil || host.guest !== m {
            host.endSheet(sheet, returnCode: .cancel)
        }
        outgoing?.slotPark(stopVoice: false)
        if let g = host.guest as? SlotMember, g !== m { g.slotPark(stopVoice: false) }
        m.slotAttach(to: host)
        if let p = m as? PopupWindow { p.setFloating(false) } else { m.slotWindow.level = .normal }
        let min = m.slotWindow.minSize
        if f.width < min.width { f.size.width = min.width }
        if f.height < min.height { f.origin.y -= min.height - f.height; f.size.height = min.height }
        frame = f
        decorate(m, v)
        controller.refreshWorkspaceStrip()
        if !hostUp && !aerospaceCacheCleared { Self.clearAerospaceCache() }
        aerospaceCacheCleared = false
        m.slotShow(frame: f)
        if outgoing != nil { swappedAt = Date() }
        current = v
        last = v
        if v.isJira { lastJira = v }
        PaneNav.shared.track(m.slotWindow)
        controller.log("shared window: \(v.rawValue)" + (stack.isEmpty ? "" : " (back: \(stack.map(\.rawValue).joined(separator: " > ")))"))
    }

    static func clearAerospaceCache() {
        guard liveAerospaceSocket() != nil else { return }
        _ = aerospaceSocket(["eval", "true"], timeout: 0.25)
    }

    func prepare(_ v: SlotView) {
        guard let m = controller.slotMember(v), !m.slotShown else { return }
        m.slotWindow.setFrame(currentFrame(), display: false)
        decorate(m, v)
    }

    func hide(_ reason: String = "", restoreFocus: Bool = true) {
        summoned = false
        swappedAt = nil
        targetScreen = nil
        aerospaceCacheCleared = false
        guard let cur = current else {
            if host.isVisible {
                host.orderOut(nil)
                controller.log("shared window: hidden" + (reason.isEmpty ? "" : " — \(reason)"))
            }
            return
        }
        current = nil
        if let m = controller.slotMember(cur) {
            if m.slotShown { frame = m.slotBaseFrame }
            m.slotPark(stopVoice: true)
        }
        if let g = host.guest as? SlotMember { g.slotPark(stopVoice: true) }
        host.orderOut(nil)
        if restoreFocus { controller.restoreFocus(wid: returnWID, pid: returnPID) }
        returnWID = nil
        returnPID = nil
        controller.log("shared window: hidden (\(cur.rawValue))" + (reason.isEmpty ? "" : " — \(reason)"))
    }

    static let escViews: [SlotView] = [.files, .notes, .ai, .jira, .confluence, .compare]

    func escapeAtTop(_ v: SlotView) {
        guard controller.escHideCount(v) > 0 else { return }
        hide("Esc (\(v.rawValue))")
    }

    func memberGone(_ v: SlotView) {
        stack.removeAll { $0 == v }
        if current == v { current = nil }
        if last == v { last = v.isJira ? .jira : v == .compareText ? .compare : .files }
        if lastJira == v { lastJira = .jira }
    }

    func cycle(_ dir: Int) {
        let next = current == .files ? Self.navNotes : Self.navFiles
        controller.log("cycle: current=\(current?.rawValue ?? "nil") dir=\(dir) next=\(next)")
        navClicked(next)
    }

    func prefixKey(_ e: NSEvent, in w: NSWindow) -> Bool {
        guard e.type == .keyDown else { return false }
        if let k = prefixHeldKey {
            if e.isARepeat && e.keyCode == k { return true }
            prefixHeldKey = nil
        }
        guard let cur = current, controller.slotMember(cur)?.slotWindow === w else { return false }
        if VimKeys.shared.searching(in: w) { return VimKeys.shared.handle(e, in: w) }
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        let armed = prefixArmedAt.map { Date().timeIntervalSince($0) < 1.5 } ?? false
        if mods == .control && e.keyCode == 11 {
            if armed { prefixArmedAt = nil; return false }
            prefixArmedAt = Date()
            prefixHeldKey = e.keyCode
            return true
        }
        if mods == .control, let dir = PaneDir(keyCode: e.keyCode) {
            if armed { prefixArmedAt = nil; return false }
            return PaneNav.shared.move(dir, in: w)
        }
        if !armed, VimKeys.shared.handle(e, in: w) { return true }
        guard armed else { return (w.firstResponder as? PopupTabsBar)?.handleNavKey(e) ?? false }
        prefixArmedAt = nil
        prefixHeldKey = e.keyCode
        switch e.charactersIgnoringModifiers?.lowercased() ?? "" {
        case "l": navClicked(previousNav ?? (current == .files ? Self.navNotes : Self.navFiles))
        case "w": controller.showViewSwitcher()
        case "t":
            if let note = controller.slotMember(cur) as? PopupWindow, note.toggleProseFromPrefix() { break }
            controller.toggleTerminalPanel()
        case "b": _ = PopupTabsBar.toggleRail(in: w)
        default: break
        }
        return true
    }

    func navClicked(_ id: Int) {
        switch id {
        case Self.navNotes: current == .notes ? () : open(.notes)
        case Self.navFiles: current == .files ? () : controller.slotShowFiles()
        case Self.navJira: current?.isJira == true ? home() : hotkey(.jira)
        case Self.navConfluence: current == .confluence ? () : open(.confluence)
        case Self.navAI: current == .ai ? () : open(.ai)
        case Self.navCompare: current == .compare ? () : open(.compare)
        case Self.navHome: home()
        case Self.navBack: back()
        case PopupChrome.workspaceBase...: controller.switchWorkspace(cell: id - PopupChrome.workspaceBase)
        default: break
        }
    }

    static var navIcons: [(image: NSImage, id: Int, tip: String)] {
        [(notesNavIcon, navNotes, "Notes"), (filesNavIcon, navFiles, "Files")]
            + [(jiraNavIcon, navJira, "Jira")]
            + (confluenceEnabled() ? [(confluenceNavIcon, navConfluence, "Confluence search")] : [])
            + (compareEnabled() ? [(compareNavIcon, navCompare, "Compare")] : [])
            + (aiEnabled() ? [(aiNavIcon, navAI, "AI view")] : [])
    }

    static func navButtons(for v: SlotView) -> [(String, Int)] {
        v == .config ? [] :
        (v.isSub ? [("back", navBack)] : [])
            + (v.isJira && v.isSub ? [("home", navHome)] : [])
    }

    static func navOn(_ v: SlotView) -> Int? {
        switch v {
        case .notes: return navNotes
        case .files: return navFiles
        case .confluence: return navConfluence
        case .ai: return navAI
        case .compare, .compareText: return navCompare
        case _ where v.isJira: return navJira
        default: return nil
        }
    }

    private func decorate(_ m: SlotMember, _ v: SlotView) {
        if let w = m as? PopupWindow {
            w.hostHandlesFocusLoss = true
            if w.navIcons.isEmpty {
                let nav = Self.navButtons(for: v)
                w.headerButtons = w.headerButtons + nav
                if let order = w.headerOrder { w.headerOrder = order + nav.map(\.1) }
                let prev = w.onHeaderButton
                w.onHeaderButton = { [weak self] id in
                    if Self.navIDs.contains(id) || id >= PopupChrome.workspaceBase { self?.navClicked(id) } else { prev?(id) }
                }
                w.onCloseWindow = { [weak self] in self?.hide("✕ / Cmd+W") }
            }
            w.navIcons = Self.navIcons
            w.headerIcon = appIcon
            w.navOn = Self.navOn(v)
            w.onCycleView = nil
        } else if let c = m as? CardWindowController, let on = Self.navOn(v) {
            c.setSlotNav(Self.navButtons(for: v), icons: Self.navIcons, icon: appIcon, on: on) { [weak self] id in
                self?.navClicked(id)
            }
            c.onCycleView = nil
            c.onSlotHide = { [weak self] in self?.hide("✕ / Cmd+W (\(v.rawValue))") }
        }
    }
}
