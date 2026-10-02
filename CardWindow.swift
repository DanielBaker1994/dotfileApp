import AppKit

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
        headerTracker.track(event, in: self, band: headerBand) { p in
            self.onHeaderClick?(NSPoint(x: p.x, y: self.frame.height - p.y))
        }
        super.sendEvent(event)
    }
}

// What the three windows share: the window + its header, the shared
// window's hooks, the common keys and the kitchen sink's head. Subclasses
// build their content into `themedRoot` and override the hooks below.
class CardWindowController: NSObject, NSWindowDelegate {
    weak var controller: SwitcherController?
    let window: CardNSWindow
    var chrome: PopupChrome?
    var monitor: Any?
    // the shared window's hooks (nil = a standalone window): ✕ / Cmd+W hide
    // the whole shared window, Ctrl+Tab / Ctrl+Shift+Tab = next / previous view
    var onSlotHide: (() -> Void)?
    var onCycleView: ((Int) -> Void)?
    private var slotNavClick: ((Int) -> Void)?

    init(controller: SwitcherController, frame: NSRect, title: String, minSize: NSSize) {
        self.controller = controller
        window = CardNSWindow(contentRect: frame,
                              styleMask: [.titled, .closable, .resizable, .miniaturizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
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
        if let hide = onSlotHide { hide() } else { window.orderOut(nil) }
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
        ch.headerTitle = title
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
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self else { return e }
            if self.keyBeforeSheet(e) { return nil }
            if let sheet = self.window.attachedSheet {
                return sheet.isKeyWindow && JiraEditKeys.route(e, in: sheet) ? nil : e
            }
            guard self.window.isKeyWindow else { return e }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command)
            if mods.contains(.control) && !cmd && e.keyCode == 48, let cycle = self.onCycleView {
                cycle(mods.contains(.shift) ? -1 : 1)                        // Ctrl+Tab: next view
                return nil
            }
            if cmd && e.keyCode == 13 { self.closeOrHide(); return nil }     // Cmd+W
            if self.handleKey(e) { return nil }
            return JiraEditKeys.route(e, in: self.window) ? nil : e
        }
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
    func slotPark(stopVoice: Bool) { window.orderOut(nil) }
    func slotShow(frame: NSRect?) {
        if let f = frame { window.setFrame(f, display: false) }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        didShow()
    }
}

extension CardWindowController: SlotMember {}
