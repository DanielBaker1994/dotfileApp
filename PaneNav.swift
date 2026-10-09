import AppKit

// Ctrl+H / J / K / L: move the keyboard to the pane on that side, in every
// view of the shared window (tmux / herdr style), and the thin silver ring
// that shows which pane has it. The geometry lives in PaneGeometry.swift
// (tested); this file is the AppKit half.
//
// A view lists its panes (`PaneProvider.navPanes`, only what is on screen
// counts); `SharedWindow.prefixKey` hands plain Ctrl+H/J/K/L here, after the
// Ctrl+B prefix had its turn (Ctrl+B Ctrl+H … = the pane gets the real key).
// No pane that way = nothing happens, the key is still used (Ctrl+H never
// turns into a surprise backspace).

struct NavPane {
    let id: String
    // the area: its frame is the pane's rect and where the ring goes
    let view: NSView
    // the area inside `view` when the pane is only part of it (Compare's
    // two sides share one document view); in `view`'s coordinates
    var part: (() -> NSRect)? = nil
    // what takes the keyboard (default: the view itself)
    var focus: (() -> Void)? = nil
    // does this responder belong to the pane? (default: the view or inside it)
    var owns: ((NSResponder) -> Bool)? = nil
    // the pane may use the key itself first (the nvim pane's own splits):
    // true = done, no jump
    var intercept: ((PaneDir) -> Bool)? = nil
    // vim mode (VimKeys.swift): what normal mode drives (nil = found under
    // `view`), what `i` / `a` focus (the pane's text input) and where Esc in
    // that input goes (normal mode); nil = not offered
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

    // a responder inside `v` — a field editor counts for the field it edits
    static func inside(_ r: NSResponder, _ v: NSView) -> Bool {
        if r === v { return true }
        if let tv = r as? NSTextView, tv.isFieldEditor, let f = tv.delegate as? NSView {
            return f === v || f.isDescendant(of: v)
        }
        if let rv = r as? NSView { return rv.isDescendant(of: v) }
        return false
    }

    // the first view under v that takes the keyboard (depth first, in
    // subview order; text fields before anything else in the same parent)
    static func firstFocusable(in v: NSView) -> NSView? {
        if v.acceptsFirstResponder, !v.isHiddenOrHasHiddenAncestor, !(v is NSButton) { return v }
        for sub in v.subviews where !sub.isHidden {
            if let f = firstFocusable(in: sub) { return f }
        }
        return nil
    }

    // a pane focused on the first field / text view inside it
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

    // the pane's rect in window coordinates (bottom-up)
    func windowRect() -> NSRect {
        // the area itself (panes are the outer views: a scroll view, not
        // its document; visibleRect is unreliable while the window is parked)
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
    // the view's panes right now, in any order (hidden ones are dropped)
    var navPanes: [NavPane] { get }
    // the keyboard moved to another pane (the view's own bookkeeping)
    func paneFocusMoved()
}
extension PaneProvider {
    func paneFocusMoved() {}
}

final class PaneNav {
    static let shared = PaneNav()

    // [app] pane-focus-color / pane-focus-width (parseAppConfig)
    static let defaultRingColor = NSColor(srgbRed: 0xc8 / 255.0, green: 0xce / 255.0, blue: 0xd8 / 255.0, alpha: 0.55)
    static var ringColor = defaultRingColor { didSet { shared.refreshAll() } }
    static var ringWidth: CGFloat = 1 { didSet { shared.refreshAll() } }

    // the window → its view's panes (set by the controller: the shared
    // window's current member, nil for anything else)
    var provider: ((NSWindow) -> PaneProvider?)?

    // the way back per view (tmux: Ctrl+L after Ctrl+H returns where you were)
    private var came: [ObjectIdentifier: [String: [PaneDir: String]]] = [:]
    private var tracked: [ObjectIdentifier: (window: NSWindow, kvo: NSKeyValueObservation)] = [:]
    private var monitors: [Any] = []
    private var refreshQueued = Set<ObjectIdentifier>()

    private init() {}

    // the visible panes of w's view, top-down for the geometry
    private func panes(in w: NSWindow, _ p: PaneProvider) -> [NavPane] {
        p.navPanes.filter { $0.visible(in: w) }
    }

    private func topDown(_ r: NSRect, in w: NSWindow) -> CGRect {
        let h = w.contentView?.bounds.height ?? w.frame.height
        return CGRect(x: r.minX, y: h - r.maxY, width: r.width, height: r.height)
    }

    // the pane with the keyboard in w (VimKeys)
    func currentPane(in w: NSWindow) -> NavPane? {
        guard let p = provider?(w) else { return nil }
        return current(in: w, panes(in: w, p))
    }

    func current(in w: NSWindow, _ list: [NavPane]) -> NavPane? {
        guard let fr = w.firstResponder else { return nil }
        // the innermost pane wins (a field inside a bigger area)
        return list.filter { $0.contains(fr) }.min { a, b in
            let ra = a.windowRect(), rb = b.windowRect()
            return ra.width * ra.height < rb.width * rb.height
        }
    }

    // Ctrl+H/J/K/L in w. Returns true when the key was used (always, when
    // the window has a pane list: no pane that way is still "used").
    func move(_ dir: PaneDir, in w: NSWindow) -> Bool {
        guard let p = provider?(w) else { return false }
        let list = panes(in: w, p)
        guard !list.isEmpty else { return false }
        let key = ObjectIdentifier(p)
        guard let cur = current(in: w, list) else {
            // nothing focused yet: the biggest pane (the view's main area)
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

    // move straight to a pane by id (the sidebar's Return / Esc, test hooks)
    @discardableResult
    func focus(_ id: String, in w: NSWindow) -> Bool {
        guard let p = provider?(w), let t = panes(in: w, p).first(where: { $0.id == id }) else { return false }
        t.takeFocus(in: w)
        p.paneFocusMoved()
        refreshSoon(w)
        return true
    }

    // the pane on `dir`'s side of `id` (the sidebar: Return → the pane right of it)
    func neighbour(of id: String, _ dir: PaneDir, in w: NSWindow) -> String? {
        guard let p = provider?(w) else { return nil }
        let list = panes(in: w, p)
        let rects = list.map { PaneRect(id: $0.id, rect: topDown($0.windowRect(), in: w)) }
        return PaneGeometry.next(from: id, dir, panes: rects, came: came[ObjectIdentifier(p)] ?? [:])
    }

    // MARK: the ring

    // Start following w: first-responder changes, key / resign, resizes, and
    // after every click or key in it (drawers opening, the sidebar rail).
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

    // where the ring is now (test state): window rect top-down, nil = hidden
    private(set) var ringState: [ObjectIdentifier: (pane: String, rect: CGRect)] = [:]

    func refresh(_ w: NSWindow) {
        guard let root = w.contentView else { return }
        let ring = root.subviews.compactMap { $0 as? PaneFocusRing }.first ?? {
            let r = PaneFocusRing(frame: .zero)
            root.addSubview(r, positioned: .above, relativeTo: nil)
            return r
        }()
        // other rings (a view's content moved here with its own) go
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
        // the vim mode chip: bottom-right of the focused pane
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
        // stay on top of overlays added since (the badge above the ring)
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

    // the test hooks' view of w: panes, the focused one, the ring
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

// the ring itself: a hairline around the focused pane, never takes clicks
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
