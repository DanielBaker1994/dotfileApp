import AppKit
import WebKit

// MARK: - Card windows (Confluence, AI, Jira Config)
//
// Titled windows (sheets, native resize) dressed like the popup cards: the
// titlebar hidden, blur + card tint + border, and the popups' own header
// strip (✕ · kitchen sink · view icons · title). In the shared window they
// are views like the popups (SlotMember, SharedWindow.decorate); with
// `[app] shared-window = false` each is an ordinary window.

// The window: tells the window server the card's corner radius (else the
// system's larger rounded frame peeks out around it) and catches header
// clicks — the invisible titlebar swallows them (as in PopupBaseWindow);
// drags stay native.
final class CardNSWindow: NSWindow {
    var cornerRadius: CGFloat = 10
    @objc func _cornerRadius() -> CGFloat { cornerRadius }
    var headerBand: CGFloat = 0
    var onHeaderClick: ((NSPoint) -> Void)?    // flipped (top-down) window coords
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

// What the three windows share: the window + its header, the shared
// window's hooks, the common keys and the kitchen sink's head. Subclasses
// build their content into `themedRoot` and override the hooks below.
class CardWindowController: NSObject, NSWindowDelegate {
    weak var controller: SwitcherController?
    // the window holding the content: `homeWindow`, or the shared window's
    // host while this view is shown in it (slotAttach / slotDetach)
    private(set) var window: NSWindow
    let homeWindow: CardNSWindow
    var chrome: PopupChrome?
    var monitor: Any?
    // the shared window's hooks (nil = a standalone window): ✕ / Cmd+W hide
    // the whole shared window, Ctrl+Tab / Ctrl+Shift+Tab = next / previous view
    var onSlotHide: (() -> Void)?
    var onCycleView: ((Int) -> Void)?
    // the themed Cmd+/ card (`showShortcutsCard`), over the whole window
    var shortcutsCard: ShortcutsOverlay?
    // the themed question over the window (`confirm`), instead of an NSAlert
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

    // MARK: hooks

    // a key the window's own (true = used); runs after the shared keys
    func handleKey(_ e: NSEvent) -> Bool { false }
    // a key that must be seen before anything else, even while a popover
    // (not this window) is key
    func keyBeforeSheet(_ e: NSEvent) -> Bool { false }
    // the header icon (kitchen sink) was clicked
    func showIconMenu() {}
    // the shared window just showed this view (focus, refresh…)
    func didShow() {}

    // ✕ / Cmd+W: hide the shared window (this view stays as is);
    // standalone: just this window
    func closeOrHide() {
        if let hide = onSlotHide { hide() } else { leaveWindow() }
    }

    // this view's window goes away: out of the shared window's host (which
    // hides itself once nothing is in it) or its own window ordered out
    func leaveWindow() {
        slotDetach()
        window.orderOut(nil)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        closeOrHide()
        return false
    }

    // MARK: surface

    // the popup windows' surface around `content`: blur + card tint +
    // border, rounded like them, with their header strip on top. `minTint`
    // = the card's least opacity (a window that is all form text wants it
    // denser)
    func themedRoot(_ content: NSView, name: String, colors: PopupColors, headerColor: NSColor,
                    icon: NSImage, title: String, minTint: CGFloat = 0.94) -> NSView {
        var cfg = PopupConfig(name: name)
        cfg.colors = colors
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = headerColor
        let radius = cfg.cornerRadius + 1
        let window = homeWindow     // the chrome's truth; a host re-takes it below
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
        // no title in the header bar (the view switcher already says where
        // you are; the content names itself) — `title` stays the window's
        ch.headerTitle = nil
        ch.copyPathLabel = ""
        ch.copyConfigLabel = ""
        chrome = ch
        for v in [fx, tint, ch, content] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        // pin the blur + tint WITHOUT re-adding them: addSubview on a view
        // that's already a child moves it to the TOP, which buried the
        // header and every control under the near-opaque tint (a blank card)
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
        // every themed control in the content takes the window palette
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

    // the shared window's header: the view icons (this one lit) + the
    // view's own nav buttons (Jira Config: back / home)
    func setSlotNav(_ buttons: [(String, Int)], icons: [(image: NSImage, id: Int, tip: String)],
                    icon: NSImage, on: Int, click: @escaping (Int) -> Void) {
        chrome?.extraButtons = buttons
        chrome?.navIcons = icons
        chrome?.navOn = on
        chrome?.headerIcon = icon
        chrome?.needsDisplay = true
        slotNavClick = click
    }

    // MARK: kitchen sink

    // its head: Global Window Options ▸ and, in the shared window, the
    // view's "Esc Hides Window"
    func iconMenu(view: SlotView) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false   // keeps a greyed global item greyed
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

    // MARK: keys (rule.md #1: edit shortcuts in every field)

    // A sheet is its own key window (edit keys only); Ctrl+Tab cycles the
    // shared window's views; Cmd+W hides; then the window's own keys, then
    // the edit-key routing.
    private func installKeys() {
        // NOT `self?.routeKey(e) ?? e`: that turned every "used" (nil)
        // back into the event, so swallowed keys still reached the field
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self else { return e }
            return self.routeKey(e)
        }
    }

    // the key monitor's whole path (nil = used); test hooks feed it too
    func routeKey(_ e: NSEvent) -> NSEvent? {
        if let c = confirmCard, window.isKeyWindow {                     // a question owns the keys
            if c.handleKey(e.keyCode, e.modifierFlags.intersection(.deviceIndependentFlagsMask)) { return nil }
            return editKey(e)                                             // its text field: typing + edit keys
        }
        if window.isKeyWindow, window.attachedSheet == nil,                 // Ctrl+B prefix
           let ic = PopupWindow.keyInterceptor, ic(e, window) { return nil }
        if keyBeforeSheet(e) { return nil }
        if let sheet = window.attachedSheet {
            return sheet.isKeyWindow ? editKey(e, in: sheet) : e
        }
        guard window.isKeyWindow else { return e }
        if let card = shortcutsCard {                                     // the card owns the keys
            _ = card.handleKey(e.keyCode, e.modifierFlags.intersection(.deviceIndependentFlagsMask))
            return nil
        }
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command)
        if mods.contains(.control) && !cmd && e.keyCode == 48, let cycle = onCycleView {
            cycle(mods.contains(.shift) ? -1 : 1)                        // Ctrl+Tab: next view
            return nil
        }
        if cmd && e.keyCode == 13 { closeOrHide(); return nil }          // Cmd+W
        if cmd && !mods.contains(.shift) && e.keyCode == 42,
           PopupTabsBar.toggleRail(in: window) { return nil }             // Cmd+\\: sidebar ⇄ icon rail
        if handleKey(e) { return nil }
        return editKey(e)
    }

    // rule.md #1 for a text field in the card. Cmd+A/X/C/V: the app's Edit
    // menu (NSText selectors) answers them BEFORE this monitor runs (the app
    // is active while a card window is key), so routing them too pasted
    // TWICE — they pass through. Cmd+Z stays routed: the menu's Undo targets
    // UndoManager.undo, which no responder answers. Ctrl+C/V: no menu item.
    func editKey(_ e: NSEvent, in w: NSWindow? = nil) -> NSEvent? {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if mods.contains(.command), !mods.contains(.control), NSApp.isActive,
           [0, 7, 8, 9].contains(e.keyCode) { return e }
        return JiraEditKeys.route(e, in: w ?? window) ? nil : e
    }

    // a web preview has focus: Cmd+C / Cmd+A like any document
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

    // MARK: SlotMember — parked = ordered out; shown = at `frame`, key

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

// MARK: - Keyboard shortcuts card

extension CardWindowController {
    // commands.toml [shortcuts] rows as the shared themed card (the one the
    // notes / files / jira views show); empty groups are dropped
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

// MARK: - Confirm card

// A question asked INSIDE the card, in the theme (an NSAlert sheet is drawn
// by the system: its own colors, its own blue default). The default button
// is the accent-filled one; Tab / ← → move the focus ring, Return / Space
// press the focused button, Esc = `cancel`. A click outside does nothing:
// the answer has to be a button.
final class ConfirmOverlay: NSView {
    typealias Choice = (title: String, role: ThemedPushButton.Role)
    // the card's own fill (callers building an accessory measure against it)
    static func fill(_ c: PopupColors) -> NSColor {
        let b = ButtonStyle.opaque(c.background)
        return b.blended(withFraction: 0.25, of: .black) ?? b
    }
    let input: JiraInputBox?                            // a one-line answer (rename, new folder)
    private let buttons: [ThemedPushButton]
    private let cancelIndex: Int
    private var focusIndex: Int { didSet { for (i, b) in buttons.enumerated() { b.keyFocus = i == focusIndex } } }
    private var answer: ((Int) -> Void)?
    private var targets: [ClosureTarget] = []          // buttons don't retain their targets

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
        // buttons right-aligned, in the order given (the caller puts the
        // default last, where macOS keeps it)
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
        buttons[defaultIndex].keyFocus = true             // didSet doesn't run inside init
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

    // true = used; false = a key for the text field (typing, edit keys)
    func handleKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let n = buttons.count
        if input != nil {
            switch code {
            case 53: cancel()                                              // Esc
            case 36, 76: finish(focusIndex)                                // Return / Enter
            case 48: break                                                 // Tab stays in the card
            default: return false
            }
            return true
        }
        switch code {
        case 53: cancel()                                                  // Esc
        case 36, 76, 49: finish(focusIndex)                                // Return / Enter / Space
        case 48: focusIndex = (focusIndex + (mods.contains(.shift) ? n - 1 : 1)) % n   // Tab
        case 123: focusIndex = max(0, focusIndex - 1)                      // ←
        case 124: focusIndex = min(n - 1, focusIndex + 1)                  // →
        default: break                                                     // swallowed
        }
        return true
    }
}

extension CardWindowController {
    // a one-line answer in the card (rename, new folder): `text` comes in
    // with its stem selected; `then` gets the answer, nil when cancelled
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

    // ask in the card: `then` gets the pressed choice's index (Esc = cancelIndex)
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



// A preview's right-click menu keeps only what helps with the page (copy,
// select all, links) — WebKit's Reload / Back / Inspect have no place in a
// card, and a Ctrl+click or Ctrl+Return landing on one popped them up.
final class QuietWebView: WKWebView {
    private static let keep: Set<String> = ["WKMenuItemIdentifierCopy", "WKMenuItemIdentifierCopyLink",
                                            "WKMenuItemIdentifierOpenLink", "WKMenuItemIdentifierCopyImage"]
    override func willOpenMenu(_ menu: NSMenu, with event: NSEvent) {
        menu.items.filter { !Self.keep.contains($0.identifier?.rawValue ?? "") }.forEach { menu.removeItem($0) }
        if menu.items.isEmpty { menu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "") }
        super.willOpenMenu(menu, with: event)
    }
}
