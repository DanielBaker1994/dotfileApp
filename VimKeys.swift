import AppKit
import SwiftTerm
import WebKit

protocol VimRows: AnyObject {
    var vimCount: Int { get }
    var vimCursor: Int { get }
    var vimPage: Int { get }
    func vimText(_ row: Int) -> String
    func vimMove(to row: Int)
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])?
}

extension VimRows {
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])? { nil }

    static func shownRows(in v: NSView, count: Int, rowH: CGFloat, rect: (Int) -> NSRect)
        -> (view: NSView, rows: [(row: Int, rect: NSRect)]) {
        let vis = v.visibleRect
        guard count > 0, rowH > 0, !vis.isEmpty else { return (v, []) }
        let first = max(0, Int(vis.minY / rowH)), last = min(count - 1, Int(vis.maxY / rowH))
        guard first <= last else { return (v, []) }
        return (v, (first...last).map { ($0, rect($0)) })
    }
}

enum VimTarget {
    case rows(VimRows)
    case text(NSTextView)
    case web(WKWebView)

    static func find(in v: NSView) -> VimTarget? {
        if v.isHidden { return nil }
        if let r = v as? VimRows { return .rows(r) }
        if v is ProseWebView || v is TerminalView { return nil }
        if let w = v as? WKWebView { return .web(w) }
        if let t = v as? NSTextView { return t.isFieldEditor ? nil : .text(t) }
        if let s = v as? NSScrollView, let d = s.documentView { return find(in: d) }
        for sub in v.subviews {
            if let t = find(in: sub) { return t }
        }
        return nil
    }
}

enum VimMode: String { case normal = "NORMAL", insert = "INSERT", search = "SEARCH" }

final class VimKeys {
    static let shared = VimKeys()
    static var enabled = true
    static var showBadge = true

    private init() {}

    private var pendingG: Date?
    private var normalField: [ObjectIdentifier: (field: NSView, caret: NSColor?)] = [:]
    private var lastQuery = ""
    private var lastBack = false
    private var bar: VimSearchBar?
    var hl: VimHighlight?

    private func ownsVim(_ r: NSResponder) -> Bool {
        if r is TerminalView || r is ProseWebView { return true }
        if let v = r as? NSView {
            var p = v.superview
            while let s = p {
                if s is TerminalView || s is ProseWebView { return true }
                p = s.superview
            }
        }
        return false
    }

    private func isTextInput(_ r: NSResponder) -> Bool {
        guard let t = r as? NSTextView else { return false }
        return t.isEditable
    }

    func target(_ pane: NavPane) -> VimTarget? { pane.vim?() ?? VimTarget.find(in: pane.view) }

    func mode(_ pane: NavPane?, in w: NSWindow) -> VimMode? {
        guard Self.enabled, let pane, let fr = w.firstResponder else { return nil }
        if let b = bar, b.window === w { return b.paneID == pane.id ? .search : nil }
        if ownsVim(fr) { return nil }
        if isTextInput(fr) {
            if let f = normalField[ObjectIdentifier(w)], NavPane.inside(fr, f.field) { return .normal }
            return pane.normal != nil ? .insert : nil
        }
        return target(pane) != nil ? .normal : nil
    }

    func enterFieldNormal(_ field: NSView, in w: NSWindow) {
        let wid = ObjectIdentifier(w)
        guard normalField[wid] == nil else { return }
        let ed = w.firstResponder as? NSTextView
        normalField[wid] = (field, ed?.insertionPointColor)
        ed?.insertionPointColor = .clear
        PaneNav.shared.refreshSoon(w)
    }

    func leaveFieldNormal(_ w: NSWindow) {
        guard let f = normalField.removeValue(forKey: ObjectIdentifier(w)) else { return }
        if let ed = w.firstResponder as? NSTextView, NavPane.inside(ed, f.field) {
            ed.insertionPointColor = f.caret ?? .textColor
        }
        PaneNav.shared.refreshSoon(w)
    }

    func searching(in w: NSWindow) -> Bool { bar?.window === w }

    func testState(_ w: NSWindow) -> [String: Any] {
        var out: [String: Any] = ["last": lastQuery, "fieldNormal": normalField[ObjectIdentifier(w)] != nil]
        if let b = bar, b.window === w {
            out["open"] = true
            out["query"] = b.query
            out["status"] = b.status
            out["pane"] = b.paneID
        } else {
            out["open"] = false
        }
        if let h = hl, h.window === w {
            out["highlight"] = ["query": h.query, "pane": h.paneID, "rows": h.overlay?.marks.count ?? 0,
                                "current": h.overlay?.marks.contains { $0.current } ?? false,
                                "text": h.textCount, "web": h.webCount]
        }
        return out
    }

    func handle(_ e: NSEvent, in w: NSWindow) -> Bool {
        guard Self.enabled, e.type == .keyDown else { return false }
        if let b = bar, b.window === w { return searchKey(e, b, in: w) }
        let wid = ObjectIdentifier(w)
        if let f = normalField[wid], !(w.firstResponder.map { NavPane.inside($0, f.field) } ?? false) {
            normalField[wid] = nil
        }
        guard let pane = PaneNav.shared.currentPane(in: w), let fr = w.firstResponder, !ownsVim(fr) else {
            return false
        }
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        let fieldNormal = normalField[wid] != nil
        if isTextInput(fr), !fieldNormal {
            guard e.keyCode == 53, mods.isEmpty, let back = pane.normal else { return false }
            pendingG = nil
            back()
            PaneNav.shared.refreshSoon(w)
            return true
        }
        guard let t = target(pane) else { return false }
        if e.keyCode == 53, mods.isEmpty, let h = hl, h.window === w {
            pendingG = nil
            clearHighlight(h)
            return true
        }
        if mods == .control, e.keyCode == 2 || e.keyCode == 32 {
            pendingG = nil
            halfPage(t, down: e.keyCode == 2)
            return true
        }
        let ch = mods.isSubset(of: .shift) ? (e.characters ?? "") : ""
        if ch == "g" {
            if let p = pendingG, Date().timeIntervalSince(p) < 0.8 {
                pendingG = nil
                edge(t, bottom: false)
            } else {
                pendingG = Date()
            }
            return true
        }
        pendingG = nil
        switch ch {
        case "j": step(t, 1)
        case "k": step(t, -1)
        case "G": edge(t, bottom: true)
        case "/", "?": openSearch(pane, t, back: ch == "?", in: w)
        case "n", "N": repeatSearch(t, reverse: ch == "N", pane: pane.id, in: w)
        case "i", "a":
            guard let ins = pane.insert else { return fieldNormal }
            leaveFieldNormal(w)
            ins()
        default:
            guard fieldNormal, mods.isSubset(of: .shift) else { return false }
            if e.keyCode == 51 || e.keyCode == 117 { return true }
            guard let u = ch.unicodeScalars.first, u.value > 0x20, u.value < 0xF700 else { return false }
            return true
        }
        PaneNav.shared.refreshSoon(w)
        return true
    }

    private func lineHeight(_ t: NSTextView) -> CGFloat {
        let f = t.font ?? .systemFont(ofSize: 13)
        return ceil(f.ascender - f.descender + f.leading) + 2
    }

    private func scroll(_ t: NSTextView, to y: CGFloat) {
        guard let sv = t.enclosingScrollView else { return }
        let clip = sv.contentView
        let maxY = max(0, t.frame.height - clip.bounds.height)
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: max(0, min(maxY, y))))
        sv.reflectScrolledClipView(clip)
    }

    private func scrollBy(_ t: NSTextView, _ dy: CGFloat) {
        guard let clip = t.enclosingScrollView?.contentView else { return }
        scroll(t, to: clip.bounds.minY + dy)
    }

    private func step(_ t: VimTarget, _ n: Int) {
        switch t {
        case .rows(let r):
            guard r.vimCount > 0 else { return }
            r.vimMove(to: max(0, min(r.vimCount - 1, r.vimCursor + n)))
        case .text(let tv): scrollBy(tv, CGFloat(n) * lineHeight(tv) * 2)
        case .web(let w): w.evaluateJavaScript("window.scrollBy(0, \(n * 48))")
        }
    }

    private func halfPage(_ t: VimTarget, down: Bool) {
        let s: CGFloat = down ? 1 : -1
        switch t {
        case .rows(let r): step(t, Int(s) * max(1, r.vimPage / 2))
        case .text(let tv): scrollBy(tv, s * (tv.enclosingScrollView?.contentView.bounds.height ?? 300) / 2)
        case .web(let w): w.evaluateJavaScript("window.scrollBy(0, \(down ? "" : "-")window.innerHeight / 2)")
        }
    }

    private func edge(_ t: VimTarget, bottom: Bool) {
        switch t {
        case .rows(let r): if r.vimCount > 0 { r.vimMove(to: bottom ? r.vimCount - 1 : 0) }
        case .text(let tv): scroll(tv, to: bottom ? .greatestFiniteMagnitude : 0)
        case .web(let w):
            w.evaluateJavaScript(bottom ? "window.scrollTo(0, document.documentElement.scrollHeight)" : "window.scrollTo(0, 0)")
        }
    }

    private func openSearch(_ pane: NavPane, _ t: VimTarget, back: Bool, in w: NSWindow) {
        guard let root = w.contentView else { return }
        let b = VimSearchBar(paneID: pane.id, target: t, back: back)
        if let h = hl { clearHighlight(h) }
        switch t {
        case .rows(let r):
            b.origin = r.vimCursor
            b.texts = (0..<r.vimCount).map { r.vimText($0) }
        case .text(let tv):
            b.origin = tv.selectedRange().location
            b.savedRange = tv.selectedRange()
            b.savedY = tv.enclosingScrollView?.contentView.bounds.minY ?? 0
        case .web: break
        }
        let rect = root.convert(pane.windowRect(), from: nil)
        let h: CGFloat = 26
        b.frame = NSRect(x: rect.minX + 6, y: root.isFlipped ? rect.maxY - h - 6 : rect.minY + 6,
                         width: max(160, rect.width - 12), height: h)
        b.autoresizingMask = []
        root.addSubview(b, positioned: .above, relativeTo: nil)
        bar = b
        PaneNav.shared.refreshSoon(w)
    }

    private func closeSearch(accept: Bool, in w: NSWindow) {
        guard let b = bar else { return }
        bar = nil
        b.removeFromSuperview()
        if let h = hl, !accept || b.query.isEmpty { clearHighlight(h) }
        if accept, !b.query.isEmpty {
            lastQuery = b.query
            lastBack = b.back
        } else if !accept {
            switch b.target {
            case .rows(let r): if r.vimCount > 0 { r.vimMove(to: min(b.origin, r.vimCount - 1)) }
            case .text(let tv):
                tv.setSelectedRange(b.savedRange)
                scroll(tv, to: b.savedY)
            case .web(let web): web.evaluateJavaScript("window.getSelection().removeAllRanges()")
            }
        }
        PaneNav.shared.refreshSoon(w)
    }

    func paneChanged(in w: NSWindow, focused: String?) {
        if let h = hl, h.window === w || h.window == nil {
            if h.window == nil || focused != h.paneID || !w.isKeyWindow { clearHighlight(h) } else { paint(h) }
        }
        guard let b = bar, b.window === w || b.window == nil else { return }
        if b.window == nil || focused != b.paneID || !w.isKeyWindow { closeSearch(accept: true, in: w) }
    }

    private func searchKey(_ e: NSEvent, _ b: VimSearchBar, in w: NSWindow) -> Bool {
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        func set(_ q: String) {
            b.undo.append(b.query)
            b.query = q
            incremental(b)
        }
        switch e.keyCode {
        case 53: closeSearch(accept: false, in: w); return true
        case 36, 76: closeSearch(accept: true, in: w); return true
        case 51 where cmd || mods.contains(.option):
            set(VimSearch.dropWord(b.query)); return true
        case 51:
            if b.query.isEmpty { closeSearch(accept: false, in: w) } else { set(String(b.query.dropLast())) }
            return true
        case 125, 126:
            find(b, from: nil, reverse: e.keyCode == 126, step: true); return true
        default: break
        }
        if ctrl && !cmd {
            switch e.keyCode {
            case 45, 35: find(b, from: nil, reverse: e.keyCode == 35, step: true)
            case 13: set(VimSearch.dropWord(b.query))
            case 32: set("")
            case 4: set(String(b.query.dropLast()))
            case 9: set(b.query + (NSPasteboard.general.string(forType: .string) ?? "").oneLine)
            case 8: copy(b.query)
            default: break
            }
            return true
        }
        if cmd {
            switch e.keyCode {
            case 9: set(b.query + (NSPasteboard.general.string(forType: .string) ?? "").oneLine)
            case 8: copy(b.query)
            case 7: copy(b.query); set("")
            case 6:
                if let prev = b.undo.popLast() { b.query = prev; incremental(b) }
            case 0: break
            default: return false
            }
            return true
        }
        if let s = e.characters, let u = s.unicodeScalars.first, u.value >= 0x20, u.value < 0xF700 {
            set(b.query + s)
        }
        return true
    }

    private func copy(_ s: String) {
        guard !s.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    private func incremental(_ b: VimSearchBar) {
        guard !b.query.isEmpty else {
            b.status = ""
            if let h = hl { clearHighlight(h) }
            if case .rows(let r) = b.target, r.vimCount > 0 { r.vimMove(to: min(b.origin, r.vimCount - 1)) }
            return
        }
        find(b, from: b.origin, reverse: b.back, step: false)
    }

    private func find(_ b: VimSearchBar, from: Int?, reverse: Bool, step: Bool) {
        guard !b.query.isEmpty else { return }
        defer { if let w = b.window { highlight(b.target, query: b.query, pane: b.paneID, in: w) } }
        switch b.target {
        case .rows(let r):
            let hit = VimSearch.rows(b.texts, query: b.query, from: from ?? r.vimCursor, back: reverse,
                                     skipCurrent: step || from == nil)
            if let i = hit.row { r.vimMove(to: i) }
            b.status = hit.row == nil ? "no match" : "\(hit.index)/\(hit.count)"
        case .text(let tv):
            let hit = VimSearch.text(tv.string, query: b.query, from: from ?? tv.selectedRange().location,
                                     back: reverse, skipCurrent: step || from == nil)
            if let rg = hit.range { show(rg, in: tv) }
            b.status = hit.range == nil ? "no match" : "\(hit.index)/\(hit.count)"
        case .web(let web):
            if !step { web.evaluateJavaScript("window.getSelection().removeAllRanges()") }
            let cfg = WKFindConfiguration()
            cfg.caseSensitive = false
            cfg.wraps = true
            cfg.backwards = reverse
            web.find(b.query, configuration: cfg) { [weak b] res in
                b?.status = res.matchFound ? "match" : "no match"
            }
        }
    }

    private func show(_ rg: NSRange, in tv: NSTextView) {
        tv.setSelectedRange(rg)
        tv.scrollRangeToVisible(rg)
        tv.showFindIndicator(for: rg)
    }

    private func repeatSearch(_ t: VimTarget, reverse: Bool, pane: String, in w: NSWindow) {
        guard !lastQuery.isEmpty else { return }
        defer { highlight(t, query: lastQuery, pane: pane, in: w) }
        let back = lastBack != reverse
        switch t {
        case .rows(let r):
            let texts = (0..<r.vimCount).map { r.vimText($0) }
            if let i = VimSearch.rows(texts, query: lastQuery, from: r.vimCursor, back: back, skipCurrent: true).row {
                r.vimMove(to: i)
            } else { NSSound.beep() }
        case .text(let tv):
            let sel = tv.selectedRange()
            if let rg = VimSearch.text(tv.string, query: lastQuery, from: sel.location, back: back, skipCurrent: true).range {
                show(rg, in: tv)
            } else { NSSound.beep() }
        case .web(let web):
            let cfg = WKFindConfiguration()
            cfg.caseSensitive = false
            cfg.wraps = true
            cfg.backwards = back
            web.find(lastQuery, configuration: cfg) { _ in }
        }
    }
}

private extension String {
    var oneLine: String { components(separatedBy: .newlines).joined(separator: " ") }
}

final class VimHighlight {
    let paneID: String
    weak var window: NSWindow?
    let target: VimTarget
    var query = ""
    var overlay: VimMatchOverlay?
    var textCount = 0
    var webCount = 0
    var painted: (query: String, length: Int)?
    init(paneID: String, window: NSWindow, target: VimTarget) {
        self.paneID = paneID
        self.window = window
        self.target = target
    }
}

extension VimKeys {
    static var matchColor: NSColor { PopupThemeDefaults.colors.palette.warning }

    func highlight(_ t: VimTarget, query: String, pane: String, in w: NSWindow) {
        if let h = hl, h.paneID != pane || h.window !== w || !Self.same(h.target, t) { clearHighlight(h) }
        guard !query.isEmpty else { if let h = hl { clearHighlight(h) }; return }
        let h = hl ?? VimHighlight(paneID: pane, window: w, target: t)
        h.query = query
        hl = h
        paint(h)
    }

    private static func same(_ a: VimTarget, _ b: VimTarget) -> Bool {
        switch (a, b) {
        case (.rows(let x), .rows(let y)): return x === y || Self.rowsView(x) === Self.rowsView(y)
        case (.text(let x), .text(let y)): return x === y
        case (.web(let x), .web(let y)): return x === y
        default: return false
        }
    }
    private static func rowsView(_ r: VimRows) -> NSView? { r.vimShownRows()?.view }

    func paint(_ h: VimHighlight) {
        switch h.target {
        case .rows(let r):
            guard let shown = r.vimShownRows() else { return }
            let host = shown.view
            let o = h.overlay ?? VimMatchOverlay(frame: .zero)
            if o.superview !== host {
                o.removeFromSuperview()
                host.addSubview(o, positioned: .above, relativeTo: nil)
                o.follow(host.enclosingScrollView?.contentView)
            }
            h.overlay = o
            let frame = host.visibleRect
            o.frame = frame
            let cursor = r.vimCursor
            o.marks = shown.rows.filter { VimSearch.matches(r.vimText($0.row), h.query) }.map {
                (rect: $0.rect.offsetBy(dx: -frame.minX, dy: -frame.minY), current: $0.row == cursor)
            }
        case .text(let tv):
            let len = (tv.string as NSString).length
            if let p = h.painted, p.query == h.query, p.length == len { return }
            guard let lm = tv.layoutManager else { return }
            lm.removeTemporaryAttribute(.backgroundColor, forCharacterRange: NSRange(location: 0, length: len))
            let all = VimSearch.ranges(tv.string, query: h.query)
            let color = Self.matchColor.withAlphaComponent(0.35)
            for rg in all { lm.addTemporaryAttribute(.backgroundColor, value: color, forCharacterRange: rg) }
            h.textCount = all.count
            h.painted = (h.query, len)
        case .web(let web):
            if let p = h.painted, p.query == h.query { return }
            h.painted = (h.query, 0)
            web.evaluateJavaScript(Self.webHighlightJS(h.query)) { [weak h] res, _ in
                h?.webCount = (res as? Int) ?? 0
            }
        }
    }

    func clearHighlight(_ h: VimHighlight) {
        if hl === h { hl = nil }
        h.overlay?.removeFromSuperview()
        h.overlay = nil
        switch h.target {
        case .rows: break
        case .text(let tv):
            tv.layoutManager?.removeTemporaryAttribute(
                .backgroundColor, forCharacterRange: NSRange(location: 0, length: (tv.string as NSString).length))
        case .web(let web):
            web.evaluateJavaScript("window.CSS && CSS.highlights && CSS.highlights.delete('vimsearch')")
        }
    }

    func repaintHighlight() {
        if let h = hl { paint(h) }
    }

    private static func webHighlightJS(_ q: String) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: [q])) ?? Data("[\"\"]".utf8)
        let arg = String(data: data, encoding: .utf8) ?? "[\"\"]"
        let c = matchColor.usingColorSpace(.sRGB) ?? matchColor
        let rgba = "rgba(\(Int(c.redComponent * 255)), \(Int(c.greenComponent * 255)), \(Int(c.blueComponent * 255)), 0.35)"
        return """
        (function (q) {
          if (!window.CSS || !CSS.highlights || !document.body) return 0;
          var st = document.getElementById('__vimhl');
          if (!st) { st = document.createElement('style'); st.id = '__vimhl'; (document.head || document.body).appendChild(st); }
          st.textContent = '::highlight(vimsearch) { background-color: \(rgba); }';
          CSS.highlights.delete('vimsearch');
          var ql = q.toLowerCase(), out = [], n;
          var walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
          while ((n = walk.nextNode()) && out.length < 5000) {
            var p = n.parentNode && n.parentNode.nodeName;
            if (p === 'SCRIPT' || p === 'STYLE') continue;
            var t = n.nodeValue.toLowerCase(), i = t.indexOf(ql);
            while (i >= 0 && out.length < 5000) {
              var r = new Range();
              r.setStart(n, i);
              r.setEnd(n, Math.min(n.nodeValue.length, i + q.length));
              out.push(r);
              i = t.indexOf(ql, i + Math.max(1, ql.length));
            }
          }
          CSS.highlights.set('vimsearch', new Highlight(...out));
          return out.length;
        })(\(arg)[0])
        """
    }
}

final class VimMatchOverlay: NSView {
    var marks: [(rect: NSRect, current: Bool)] = [] { didSet { needsDisplay = true } }
    private var scrollObserver: NSObjectProtocol?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.zPosition = 100
        autoresizingMask = []
    }
    required init?(coder: NSCoder) { fatalError() }
    deinit { if let o = scrollObserver { NotificationCenter.default.removeObserver(o) } }
    override var isFlipped: Bool { superview?.isFlipped ?? true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func follow(_ clip: NSClipView?) {
        if let o = scrollObserver { NotificationCenter.default.removeObserver(o) }
        scrollObserver = nil
        guard let clip else { return }
        clip.postsBoundsChangedNotifications = true
        scrollObserver = NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification, object: clip, queue: .main) { _ in
            VimKeys.shared.repaintHighlight()
        }
    }

    override func removeFromSuperview() {
        follow(nil)
        super.removeFromSuperview()
    }

    override func draw(_ dirtyRect: NSRect) {
        let color = VimKeys.matchColor
        for m in marks {
            let r = m.rect.insetBy(dx: 1, dy: 1)
            guard r.intersects(dirtyRect) else { continue }
            let path = NSBezierPath(roundedRect: r, xRadius: 5, yRadius: 5)
            color.withAlphaComponent(m.current ? 0.40 : 0.22).setFill()
            path.fill()
            color.withAlphaComponent(m.current ? 1 : 0.7).setFill()
            NSBezierPath(roundedRect: NSRect(x: r.minX, y: r.minY + 3, width: 3, height: max(0, r.height - 6)),
                         xRadius: 1.5, yRadius: 1.5).fill()
            if m.current {
                path.lineWidth = 1.5
                color.setStroke()
                path.stroke()
            }
        }
    }
}

final class VimSearchBar: NSView {
    let paneID: String
    let target: VimTarget
    let back: Bool
    var texts: [String] = []
    var origin = 0
    var savedRange = NSRange(location: 0, length: 0)
    var savedY: CGFloat = 0
    var undo: [String] = []
    var query = "" { didSet { needsDisplay = true } }
    var status = "" { didSet { needsDisplay = true } }

    init(paneID: String, target: VimTarget, back: Bool) {
        self.paneID = paneID
        self.target = target
        self.back = back
        super.init(frame: .zero)
        wantsLayer = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        let c = PopupThemeDefaults.colors
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: 6, yRadius: 6)
        c.background.blended(withFraction: 0.12, of: c.text)?.withAlphaComponent(0.97).setFill()
        path.fill()
        PaneNav.ringColor.setStroke()
        path.lineWidth = 1
        path.stroke()
        let font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        let a: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.text]
        let lead = NSAttributedString(string: back ? "?" : "/", attributes: [.font: font, .foregroundColor: c.accent])
        let line = NSMutableAttributedString(attributedString: lead)
        line.append(NSAttributedString(string: query, attributes: a))
        line.append(NSAttributedString(string: "▏", attributes: [.font: font, .foregroundColor: c.accent]))
        let h = line.size().height
        line.draw(at: NSPoint(x: 9, y: (bounds.height - h) / 2))
        let hint = status.isEmpty ? "⏎ keep · esc back · ↑↓ prev / next" : status
        let sa: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: status == "no match" ? c.palette.danger : c.dim,
        ]
        let sz = (hint as NSString).size(withAttributes: sa)
        (hint as NSString).draw(at: NSPoint(x: bounds.width - sz.width - 10, y: (bounds.height - sz.height) / 2),
                                withAttributes: sa)
    }
}

final class VimModeBadge: NSView {
    var mode: VimMode = .normal { didSet { needsDisplay = true } }
    override init(frame: NSRect) {
        super.init(frame: frame)
        isHidden = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    static let font = NSFont.monospacedSystemFont(ofSize: 9, weight: .semibold)
    static func size(_ m: VimMode) -> NSSize {
        let s = (m.rawValue as NSString).size(withAttributes: [.font: font])
        return NSSize(width: ceil(s.width) + 12, height: 15)
    }
    override func draw(_ dirtyRect: NSRect) {
        let c = PopupThemeDefaults.colors
        let fill: NSColor = mode == .insert ? c.accent.withAlphaComponent(0.85) : PaneNav.ringColor.withAlphaComponent(0.35)
        fill.setFill()
        NSBezierPath(roundedRect: bounds, xRadius: bounds.height / 2, yRadius: bounds.height / 2).fill()
        let a: [NSAttributedString.Key: Any] = [.font: Self.font, .foregroundColor: mode == .insert ? c.onAccent : c.text]
        let s = (mode.rawValue as NSString).size(withAttributes: a)
        (mode.rawValue as NSString).draw(at: NSPoint(x: (bounds.width - s.width) / 2, y: (bounds.height - s.height) / 2),
                                         withAttributes: a)
    }
}
