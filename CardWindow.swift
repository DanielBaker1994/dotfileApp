import AppKit
import WebKit

final class CardNSWindow: NSWindow {
    var cornerRadius: CGFloat = 10
    @objc func _cornerRadius() -> CGFloat { cornerRadius }
    var headerBand: CGFloat = 0
    var onHeaderClick: ((NSPoint) -> Void)?
    private var headerTracker = HeaderClickTracker()
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
    override func sendEvent(_ event: NSEvent) {
        if let p = headerTracker.track(event, in: self, band: headerBand) {
            onHeaderClick?(NSPoint(x: p.x, y: frame.height - p.y))
        }
        super.sendEvent(event)
    }
}

class CardWindowController: NSObject, NSWindowDelegate {
    weak var controller: SwitcherController?
    private(set) var window: NSWindow
    let homeWindow: CardNSWindow
    var chrome: PopupChrome?
    var monitor: Any?
    var onSlotHide: (() -> Void)?
    var onCycleView: ((Int) -> Void)?
    var shortcutsCard: ShortcutsOverlay?
    var confirmCard: ConfirmOverlay?
    private var slotNavClick: ((Int) -> Void)?

    init(controller: SwitcherController, frame: NSRect, title: String, minSize: NSSize) {
        self.controller = controller
        homeWindow = CardNSWindow(contentRect: frame,
                                  styleMask: [.titled, .closable, .resizable, .miniaturizable, .fullSizeContentView],
                                  backing: .buffered, defer: false)
        window = homeWindow
        super.init()
        window.title = title
        window.isReleasedWhenClosed = false
        window.minSize = minSize
        window.delegate = self
        installKeys()
    }

    func handleKey(_ e: NSEvent) -> Bool { false }
    func keyBeforeSheet(_ e: NSEvent) -> Bool { false }
    func showIconMenu() {}
    func didShow() {}

    func closeOrHide() {
        if let hide = onSlotHide { hide() } else { leaveWindow() }
    }

    func leaveWindow() {
        slotDetach()
        window.orderOut(nil)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        closeOrHide()
        return false
    }

    func themedRoot(_ content: NSView, name: String, colors: PopupColors, headerColor: NSColor,
                    icon: NSImage, title: String, minTint: CGFloat = 0.94) -> NSView {
        var cfg = PopupConfig(name: name)
        cfg.colors = colors
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = headerColor
        let radius = cfg.cornerRadius + 1
        let window = homeWindow
        window.cornerRadius = radius
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        for b: NSWindow.ButtonType in [.closeButton, .miniaturizeButton, .zoomButton] {
            window.standardWindowButton(b)?.isHidden = true
        }
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = true
        window.appearance = NSAppearance(named: colors.isLight ? .aqua : .darkAqua)

        let root = NSView()
        root.wantsLayer = true
        root.layer?.cornerRadius = radius
        root.layer?.masksToBounds = true
        let fx = NSVisualEffectView()
        fx.material = cfg.material
        fx.blendingMode = .behindWindow
        fx.state = .active
        let tint = NSView()
        tint.wantsLayer = true
        tint.layer?.backgroundColor = colors.base.withAlphaComponent(max(cfg.tintAlpha, minTint)).cgColor
        tint.layer?.borderColor = colors.border.cgColor
        tint.layer?.borderWidth = 1
        tint.layer?.cornerRadius = radius
        let ch = PopupChrome(config: cfg)
        ch.dragHeaderHeight = cfg.headerHeight
        ch.headerIcon = icon
        ch.headerTitle = nil
        ch.copyPathLabel = ""
        ch.copyConfigLabel = ""
        chrome = ch
        for v in [fx, tint, ch, content] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        for v in [fx, tint] as [NSView] {
            NSLayoutConstraint.activate([
                v.leadingAnchor.constraint(equalTo: root.leadingAnchor),
                v.trailingAnchor.constraint(equalTo: root.trailingAnchor),
                v.topAnchor.constraint(equalTo: root.topAnchor),
                v.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            ])
        }
        NSLayoutConstraint.activate([
            ch.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            ch.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            ch.topAnchor.constraint(equalTo: root.topAnchor),
            ch.heightAnchor.constraint(equalToConstant: cfg.headerHeight),
            content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 1),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -1),
            content.topAnchor.constraint(equalTo: ch.bottomAnchor),
            content.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -1),
        ])
        func walk(_ v: NSView) {
            (v as? PopupThemeable)?.applyColors(colors)
            v.subviews.forEach(walk)
        }
        walk(content)
        window.headerBand = cfg.headerHeight
        window.onHeaderClick = { [weak self] p in self?.headerClicked(at: p) }
        if let host = self.window as? SlotHostWindow { host.take(self, chromeOf: homeWindow) }
        return root
    }

    private func headerClicked(at p: NSPoint) {
        guard let ch = chrome else { return }
        if ch.closeButtonRect.insetBy(dx: -2, dy: -2).contains(p) {
            closeOrHide()
        } else if let hit = ch.extraButtonRects.first(where: { $0.value.contains(p) }) {
            slotNavClick?(hit.key)
        } else if ch.headerIcon != nil, ch.iconButtonRect.insetBy(dx: -4, dy: -4).contains(p) {
            showIconMenu()
        }
    }

    func setSlotNav(_ buttons: [(String, Int)], icons: [(image: NSImage, id: Int, tip: String)],
                    icon: NSImage, on: Int, click: @escaping (Int) -> Void) {
        chrome?.extraButtons = buttons
        chrome?.navIcons = icons
        chrome?.navOn = on
        chrome?.headerIcon = icon
        chrome?.needsDisplay = true
        slotNavClick = click
    }

    func iconMenu(view: SlotView) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false
        if let c = controller {
            c.addGlobalWindowItems(to: menu)
            menu.addItem(.separator())
            c.addCardThemeMenu(to: menu, view: view)
            menu.addItem(.separator())
            if onSlotHide != nil {
                menu.addItem(c.escHidesMenuItem(view))
                menu.addItem(.separator())
            }
        }
        return menu
    }

    func popUpIconMenu(_ menu: NSMenu) {
        guard let ch = chrome else { return }
        ch.iconMenuOpen = true
        menu.popUp(positioning: nil, at: NSPoint(x: ch.iconButtonRect.minX, y: ch.iconButtonRect.maxY + 4), in: ch)
        ch.iconMenuOpen = false
    }

    private func installKeys() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self else { return e }
            return self.routeKey(e)
        }
    }

    func routeKey(_ e: NSEvent) -> NSEvent? {
        if let c = confirmCard, window.isKeyWindow {
            if c.handleKey(e.keyCode, e.modifierFlags.intersection(.deviceIndependentFlagsMask)) { return nil }
            return editKey(e)
        }
        if window.isKeyWindow, window.attachedSheet == nil,
           let ic = PopupWindow.keyInterceptor, ic(e, window) { return nil }
        if keyBeforeSheet(e) { return nil }
        if let sheet = window.attachedSheet {
            return sheet.isKeyWindow ? editKey(e, in: sheet) : e
        }
        guard window.isKeyWindow else { return e }
        if let card = shortcutsCard {
            _ = card.handleKey(e.keyCode, e.modifierFlags.intersection(.deviceIndependentFlagsMask))
            return nil
        }
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command)
        if mods.contains(.control) && !cmd && e.keyCode == 48, let cycle = onCycleView {
            cycle(mods.contains(.shift) ? -1 : 1)
            return nil
        }
        if cmd && e.keyCode == 13 { closeOrHide(); return nil }
        if cmd && !mods.contains(.shift) && e.keyCode == 42,
           PopupTabsBar.toggleRail(in: window) { return nil }
        if handleKey(e) { return nil }
        return editKey(e)
    }

    func editKey(_ e: NSEvent, in w: NSWindow? = nil) -> NSEvent? {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if mods.contains(.command), !mods.contains(.control), NSApp.isActive,
           [0, 7, 8, 9].contains(e.keyCode) { return e }
        return JiraEditKeys.route(e, in: w ?? window) ? nil : e
    }

    func webEditKey(_ e: NSEvent, in web: NSView?) -> Bool {
        guard let web, e.modifierFlags.contains(.command),
              (window.firstResponder as? NSView)?.isDescendant(of: web) == true else { return false }
        switch e.keyCode {
        case 8: NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: nil)
        case 0: NSApp.sendAction(#selector(NSText.selectAll(_:)), to: nil, from: nil)
        default: return false
        }
        return true
    }

    var slotWindow: NSWindow { window }
    var slotShown: Bool { window.isVisible }
    var slotBaseFrame: NSRect { window.frame }
    func slotPark(stopVoice: Bool) { leaveWindow() }
    func slotAttach(to host: SlotHostWindow) {
        guard window === homeWindow else { return }
        host.take(self, chromeOf: homeWindow)
        host.delegate = self
        SlotHostWindow.moveContent(from: homeWindow, to: host)
        window = host
    }
    func slotDetach() {
        guard window !== homeWindow, let host = window as? SlotHostWindow else { return }
        homeWindow.setFrame(host.frame, display: false)
        SlotHostWindow.moveContent(from: host, to: homeWindow)
        if host.delegate === self { host.delegate = nil }
        window = homeWindow
        host.guestLeft(self)
    }
    func slotShow(frame: NSRect?) {
        if let f = frame { window.setFrame(f, display: false) }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        didShow()
    }
}

extension CardWindowController: SlotMember {}

extension CardWindowController {
    func showShortcutsCard(_ groups: [ShortcutsOverlay.Group], colors: PopupColors, title: String = "Keyboard Shortcuts") {
        let gs = groups.filter { !$0.items.isEmpty }
        guard let root = window.contentView, !gs.isEmpty else { return }
        closeShortcutsCard()
        let o = ShortcutsOverlay(groups: gs, colors: colors, zoom: 1, in: root, title: title)
        o.onClose = { [weak self] in self?.closeShortcutsCard() }
        shortcutsCard = o
    }

    func closeShortcutsCard() {
        shortcutsCard?.removeFromSuperview()
        shortcutsCard = nil
    }
}

final class ConfirmOverlay: NSView {
    typealias Choice = (title: String, role: ThemedPushButton.Role)
    static func fill(_ c: PopupColors) -> NSColor {
        let b = ButtonStyle.opaque(c.background)
        return b.blended(withFraction: 0.25, of: .black) ?? b
    }
    let input: JiraInputBox?
    private let buttons: [ThemedPushButton]
    private let cancelIndex: Int
    private var focusIndex: Int { didSet { for (i, b) in buttons.enumerated() { b.keyFocus = i == focusIndex } } }
    private var answer: ((Int) -> Void)?
    private var targets: [ClosureTarget] = []

    private final class Card: NSView {
        override var isFlipped: Bool { true }
        override func mouseDown(with e: NSEvent) {}
    }

    init(title: String, info: String, input: JiraInputBox? = nil, accessory: NSView?, choices: [Choice],
         defaultIndex: Int, cancelIndex: Int, colors c: PopupColors, in root: NSView, then: @escaping (Int) -> Void) {
        self.input = input
        self.cancelIndex = cancelIndex
        focusIndex = defaultIndex
        answer = then
        buttons = choices.map { ch in
            let b = ThemedPushButton(title: ch.title, target: nil, action: nil)
            b.role = ch.role
            b.colors = c
            b.isBordered = false
            return b
        }
        super.init(frame: root.bounds)
        autoresizingMask = [.width, .height]
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.3).cgColor

        let pad: CGFloat = 18, w = min(480, root.bounds.width - 40), inner = w - pad * 2
        let card = Card()
        card.wantsLayer = true
        let fill = Self.fill(c)
        card.layer?.backgroundColor = fill.withAlphaComponent(0.98).cgColor
        card.layer?.cornerRadius = 12
        card.layer?.borderColor = c.text.withAlphaComponent(0.15).cgColor
        card.layer?.borderWidth = 1
        card.layer?.shadowColor = NSColor.black.cgColor
        card.layer?.shadowOpacity = 0.4
        card.layer?.shadowRadius = 18

        func label(_ t: String, _ f: NSFont, _ col: NSColor) -> NSTextField {
            let l = NSTextField(wrappingLabelWithString: t)
            l.font = f
            l.textColor = col
            l.preferredMaxLayoutWidth = inner
            l.isSelectable = false
            let h = l.sizeThatFits(NSSize(width: inner, height: .greatestFiniteMagnitude)).height
            l.frame.size = NSSize(width: inner, height: ceil(h))
            return l
        }
        var y = pad
        let t = label(title, .systemFont(ofSize: 14, weight: .semibold), c.ensure(c.text, on: fill))
        t.frame.origin = NSPoint(x: pad, y: y)
        card.addSubview(t)
        y = t.frame.maxY + 6
        if !info.isEmpty {
            let i = label(info, .systemFont(ofSize: 12), c.ensure(c.over(c.text, 0.78, on: fill), on: fill))
            i.frame.origin = NSPoint(x: pad, y: y)
            card.addSubview(i)
            y = i.frame.maxY
        }
        if let box = input {
            y += 12
            box.colors = c
            box.frame = NSRect(x: pad, y: y, width: inner, height: JiraTheme.height)
            card.addSubview(box)
            y = box.frame.maxY
        }
        if let acc = accessory {
            y += 12
            acc.frame = NSRect(x: pad, y: y, width: inner, height: acc.frame.height)
            card.addSubview(acc)
            y = acc.frame.maxY
        }
        y += 18
        var x = w - pad
        for b in buttons.reversed() {
            let bw = max(84, b.intrinsicContentSize.width)
            x -= bw
            b.frame = NSRect(x: x, y: y, width: bw, height: 28)
            x -= 8
            card.addSubview(b)
        }
        y += 28 + pad
        card.frame = NSRect(x: ((root.bounds.width - w) / 2).rounded(),
                            y: ((root.bounds.height - y) / 2).rounded(), width: w, height: y)
        card.autoresizingMask = [.minXMargin, .maxXMargin, .minYMargin, .maxYMargin]
        addSubview(card)
        targets = buttons.indices.map { i in ClosureTarget { [weak self] in self?.finish(i) } }
        for (b, t) in zip(buttons, targets) { b.target = t; b.action = #selector(ClosureTarget.run) }
        buttons[defaultIndex].keyFocus = true
        root.addSubview(self, positioned: .above, relativeTo: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func mouseDown(with e: NSEvent) {}

    func finish(_ i: Int) {
        guard let a = answer else { return }
        answer = nil
        removeFromSuperview()
        a(i)
    }
    func cancel() { finish(cancelIndex) }

    func handleKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let n = buttons.count
        if input != nil {
            switch code {
            case 53: cancel()
            case 36, 76: finish(focusIndex)
            case 48: break
            default: return false
            }
            return true
        }
        switch code {
        case 53: cancel()
        case 36, 76, 49: finish(focusIndex)
        case 48: focusIndex = (focusIndex + (mods.contains(.shift) ? n - 1 : 1)) % n
        case 123: focusIndex = max(0, focusIndex - 1)
        case 124: focusIndex = min(n - 1, focusIndex + 1)
        default: break
        }
        return true
    }
}

extension CardWindowController {
    func prompt(_ title: String, info: String, text: String, ok: String, colors: PopupColors,
                then: @escaping (String?) -> Void) {
        let box = JiraInputBox(placeholder: "")
        box.field.stringValue = text
        confirm(title, info: info, input: box, choices: [("Cancel", .normal), (ok, .primary)],
                defaultIndex: 1, cancelIndex: 0, colors: colors) { i in
            then(i == 1 ? box.field.stringValue : nil)
        }
        window.makeFirstResponder(box.field)
        let stem = ((text as NSString).deletingPathExtension as NSString).length
        box.field.currentEditor()?.selectedRange = NSRange(location: 0, length: stem > 0 ? stem : (text as NSString).length)
    }

    func confirm(_ title: String, info: String = "", input: JiraInputBox? = nil, accessory: NSView? = nil,
                 choices: [ConfirmOverlay.Choice], defaultIndex: Int, cancelIndex: Int, colors: PopupColors,
                 then: @escaping (Int) -> Void) {
        guard let root = window.contentView else { return }
        confirmCard?.cancel()
        closeShortcutsCard()
        confirmCard = ConfirmOverlay(title: title, info: info, input: input, accessory: accessory, choices: choices,
                                     defaultIndex: defaultIndex, cancelIndex: cancelIndex, colors: colors,
                                     in: root) { [weak self] i in
            self?.confirmCard = nil
            then(i)
        }
    }
}

final class QuietWebView: WKWebView {
    private static let keep: Set<String> = ["WKMenuItemIdentifierCopy", "WKMenuItemIdentifierCopyLink",
                                            "WKMenuItemIdentifierOpenLink", "WKMenuItemIdentifierCopyImage"]
    override func willOpenMenu(_ menu: NSMenu, with event: NSEvent) {
        menu.items.filter { !Self.keep.contains($0.identifier?.rawValue ?? "") }.forEach { menu.removeItem($0) }
        if menu.items.isEmpty { menu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "") }
        super.willOpenMenu(menu, with: event)
    }
}
