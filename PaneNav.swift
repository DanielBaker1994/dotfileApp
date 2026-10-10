import AppKit

struct NavPane {
    let id: String
    let view: NSView
    var part: (() -> NSRect)? = nil
    var focus: (() -> Void)? = nil
    var owns: ((NSResponder) -> Bool)? = nil
    var intercept: ((PaneDir) -> Bool)? = nil
    var vim: (() -> VimTarget?)? = nil
    var insert: (() -> Void)? = nil
    var normal: (() -> Void)? = nil

    init(_ id: String, _ view: NSView, part: (() -> NSRect)? = nil, focus: (() -> Void)? = nil,
         owns: ((NSResponder) -> Bool)? = nil, intercept: ((PaneDir) -> Bool)? = nil) {
        self.id = id
        self.view = view
        self.part = part
        self.focus = focus
        self.owns = owns
        self.intercept = intercept
    }

    static func inside(_ r: NSResponder, _ v: NSView) -> Bool {
        if r === v { return true }
        if let tv = r as? NSTextView, tv.isFieldEditor, let f = tv.delegate as? NSView {
            return f === v || f.isDescendant(of: v)
        }
        if let rv = r as? NSView { return rv.isDescendant(of: v) }
        return false
    }

    static func firstFocusable(in v: NSView) -> NSView? {
        if v.acceptsFirstResponder, !v.isHiddenOrHasHiddenAncestor, !(v is NSButton) { return v }
        for sub in v.subviews where !sub.isHidden {
            if let f = firstFocusable(in: sub) { return f }
        }
        return nil
    }

    static func area(_ id: String, _ v: NSView) -> NavPane {
        NavPane(id, v, focus: { [weak v] in
            guard let v, let w = v.window else { return }
            w.makeFirstResponder(NavPane.firstFocusable(in: v) ?? v)
        })
    }

    func contains(_ r: NSResponder) -> Bool { owns?(r) ?? Self.inside(r, view) }

    func takeFocus(in w: NSWindow) {
        if let focus { focus() } else { w.makeFirstResponder(view) }
    }

    func windowRect() -> NSRect {
        let local = part?() ?? view.bounds
        return view.convert(local, to: nil)
    }

    func visible(in w: NSWindow) -> Bool {
        guard view.window === w, !view.isHiddenOrHasHiddenAncestor else { return false }
        let r = windowRect()
        return r.width > 4 && r.height > 4
    }
}

protocol PaneProvider: AnyObject {
    var navPanes: [NavPane] { get }
    func paneFocusMoved()
}
extension PaneProvider {
    func paneFocusMoved() {}
}

final class PaneNav {
    static let shared = PaneNav()

    static let defaultRingColor = NSColor(srgbRed: 0xc8 / 255.0, green: 0xce / 255.0, blue: 0xd8 / 255.0, alpha: 0.55)
    static var ringColor = defaultRingColor { didSet { shared.refreshAll() } }
    static var ringWidth: CGFloat = 1 { didSet { shared.refreshAll() } }

    var provider: ((NSWindow) -> PaneProvider?)?

    private var came: [ObjectIdentifier: [String: [PaneDir: String]]] = [:]
    private var tracked: [ObjectIdentifier: (window: NSWindow, kvo: NSKeyValueObservation)] = [:]
    private var monitors: [Any] = []
    private var refreshQueued = Set<ObjectIdentifier>()

    private init() {}

    private func panes(in w: NSWindow, _ p: PaneProvider) -> [NavPane] {
        p.navPanes.filter { $0.visible(in: w) }
    }

    private func topDown(_ r: NSRect, in w: NSWindow) -> CGRect {
        let h = w.contentView?.bounds.height ?? w.frame.height
        return CGRect(x: r.minX, y: h - r.maxY, width: r.width, height: r.height)
    }

    func currentPane(in w: NSWindow) -> NavPane? {
        guard let p = provider?(w) else { return nil }
        return current(in: w, panes(in: w, p))
    }

    func current(in w: NSWindow, _ list: [NavPane]) -> NavPane? {
        guard let fr = w.firstResponder else { return nil }
        return list.filter { $0.contains(fr) }.min { a, b in
            let ra = a.windowRect(), rb = b.windowRect()
            return ra.width * ra.height < rb.width * rb.height
        }
    }

    func move(_ dir: PaneDir, in w: NSWindow) -> Bool {
        guard let p = provider?(w) else { return false }
        let list = panes(in: w, p)
        guard !list.isEmpty else { return false }
        let key = ObjectIdentifier(p)
        guard let cur = current(in: w, list) else {
            list.max { a, b in
                let ra = a.windowRect(), rb = b.windowRect()
                return ra.width * ra.height < rb.width * rb.height
            }?.takeFocus(in: w)
            refreshSoon(w)
            return true
        }
        if cur.intercept?(dir) == true { return true }
        let rects = list.map { PaneRect(id: $0.id, rect: topDown($0.windowRect(), in: w)) }
        var memo = came[key] ?? [:]
        if let to = PaneGeometry.next(from: cur.id, dir, panes: rects, came: memo),
           let target = list.first(where: { $0.id == to }) {
            PaneGeometry.remember(&memo, from: cur.id, to: to, dir)
            came[key] = memo
            target.takeFocus(in: w)
            p.paneFocusMoved()
        }
        refreshSoon(w)
        return true
    }

    @discardableResult
    func focus(_ id: String, in w: NSWindow) -> Bool {
        guard let p = provider?(w), let t = panes(in: w, p).first(where: { $0.id == id }) else { return false }
        t.takeFocus(in: w)
        p.paneFocusMoved()
        refreshSoon(w)
        return true
    }

    func neighbour(of id: String, _ dir: PaneDir, in w: NSWindow) -> String? {
        guard let p = provider?(w) else { return nil }
        let list = panes(in: w, p)
        let rects = list.map { PaneRect(id: $0.id, rect: topDown($0.windowRect(), in: w)) }
        return PaneGeometry.next(from: id, dir, panes: rects, came: came[ObjectIdentifier(p)] ?? [:])
    }

    func track(_ w: NSWindow) {
        let id = ObjectIdentifier(w)
        if tracked[id] == nil {
            let kvo = w.observe(\.firstResponder, options: []) { [weak self] win, _ in
                self?.refreshSoon(win)
            }
            tracked[id] = (w, kvo)
            let nc = NotificationCenter.default
            for n in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification,
                      NSWindow.didResizeNotification] {
                nc.addObserver(forName: n, object: w, queue: .main) { [weak self, weak w] _ in
                    if let w { self?.refreshSoon(w) }
                }
            }
        }
        if monitors.isEmpty {
            let mask: NSEvent.EventTypeMask = [.leftMouseUp, .keyDown]
            if let m = NSEvent.addLocalMonitorForEvents(matching: mask, handler: { [weak self] e in
                if let w = e.window, self?.tracked[ObjectIdentifier(w)] != nil { self?.refreshSoon(w) }
                return e
            }) { monitors.append(m) }
        }
        refreshSoon(w)
    }

    func refreshSoon(_ w: NSWindow) {
        let id = ObjectIdentifier(w)
        guard !refreshQueued.contains(id) else { return }
        refreshQueued.insert(id)
        DispatchQueue.main.async { [weak self, weak w] in
            self?.refreshQueued.remove(id)
            if let w { self?.refresh(w) }
        }
    }

    func refreshAll() {
        for t in tracked.values { refresh(t.window) }
    }

    private(set) var ringState: [ObjectIdentifier: (pane: String, rect: CGRect)] = [:]

    func refresh(_ w: NSWindow) {
        guard let root = w.contentView else { return }
        let ring = root.subviews.compactMap { $0 as? PaneFocusRing }.first ?? {
            let r = PaneFocusRing(frame: .zero)
            root.addSubview(r, positioned: .above, relativeTo: nil)
            return r
        }()
        for extra in root.subviews.compactMap({ $0 as? PaneFocusRing }) where extra !== ring {
            extra.removeFromSuperview()
        }
        let badges = root.subviews.compactMap { $0 as? VimModeBadge }
        for extra in badges.dropFirst() { extra.removeFromSuperview() }
        let badge = root.subviews.compactMap { $0 as? VimModeBadge }.first ?? {
            let b = VimModeBadge(frame: .zero)
            root.addSubview(b, positioned: .above, relativeTo: nil)
            return b
        }()
        let wid = ObjectIdentifier(w)
        guard w.isKeyWindow, w.attachedSheet == nil, let p = provider?(w) else {
            ring.isHidden = true
            badge.isHidden = true
            ringState[wid] = nil
            VimKeys.shared.paneChanged(in: w, focused: nil)
            return
        }
        let list = panes(in: w, p)
        let focused = current(in: w, list)
        VimKeys.shared.paneChanged(in: w, focused: focused?.id)
        if VimKeys.showBadge, let cur = focused, let m = VimKeys.shared.mode(cur, in: w), m != .search {
            let r = root.convert(cur.windowRect(), from: nil)
            let sz = VimModeBadge.size(m)
            badge.mode = m
            badge.frame = NSRect(x: r.maxX - sz.width - 8, y: root.isFlipped ? r.maxY - sz.height - 6 : r.minY + 6,
                                 width: sz.width, height: sz.height)
            if root.subviews.last !== badge { root.addSubview(badge, positioned: .above, relativeTo: nil) }
            badge.isHidden = false
        } else {
            badge.isHidden = true
        }
        guard list.count > 1, let cur = focused else {
            ring.isHidden = true
            ringState[wid] = nil
            return
        }
        if root.subviews.last !== ring, root.subviews.last !== badge {
            root.addSubview(ring, positioned: .above, relativeTo: nil)
            if !badge.isHidden { root.addSubview(badge, positioned: .above, relativeTo: nil) }
        }
        let wr = cur.windowRect()
        ring.frame = root.convert(wr, from: nil).insetBy(dx: 0.5, dy: 0.5)
        ring.apply(color: Self.ringColor, width: Self.ringWidth)
        ring.isHidden = false
        ringState[wid] = (cur.id, topDown(wr, in: w))
    }

    func testState(_ w: NSWindow) -> [String: Any] {
        guard let p = provider?(w) else { return [:] }
        let list = panes(in: w, p)
        let cur = current(in: w, list)
        let ring = ringState[ObjectIdentifier(w)]
        func box(_ r: CGRect) -> [Int] { [Int(r.minX), Int(r.minY), Int(r.width), Int(r.height)] }
        return [
            "focused": cur?.id ?? "",
            "vimMode": VimKeys.shared.mode(cur, in: w)?.rawValue ?? "",
            "vimSearch": VimKeys.shared.testState(w),
            "panes": list.map { ["id": $0.id, "rect": box(topDown($0.windowRect(), in: w))] },
            "ring": ring.map { ["pane": $0.pane, "rect": box($0.rect)] as [String: Any] } ?? ["pane": ""],
        ]
    }
}

final class PaneFocusRing: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 6
        isHidden = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func apply(color: NSColor, width: CGFloat) {
        layer?.borderColor = color.cgColor
        layer?.borderWidth = width
    }
}
