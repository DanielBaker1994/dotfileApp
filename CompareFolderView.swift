import AppKit
import Quartz

// Folder Compare's view (PRD-compare.md §7.1 wireframe C): the toolbar
// (filters, flatten, name filter), two path headers, ONE flipped document
// view that draws both trees + the center glyph column (synced by
// construction), the status line. The engine is CompareFolder.swift; the
// window (CompareWindow.swift) owns the sessions and the shared-window
// behavior (Esc chain, dismissal) and talks to this through `FolderHost`.

protocol FolderHost: AnyObject {
    var folderWindow: NSWindow { get }
    var folderColors: PopupColors { get }
    var folderConfig: CompareConfig { get }
    func folderOpenPair(_ left: String?, _ right: String?)     // a file pair → Text Compare (a back step)
    func folderChanged()                                       // pill label / badges
    func folderLog(_ s: String)
    func folderOpenNote(_ path: String)
    func folderSwapped(old: FolderSession, new: FolderSession)   // new roots (swap, base folder, back / up): the session's pair changes
    // a themed question in the window (`CardWindowController.confirm`)
    func folderConfirm(_ title: String, info: String, accessory: NSView?, choices: [ConfirmOverlay.Choice],
                       defaultIndex: Int, cancelIndex: Int, then: @escaping (Int) -> Void)
    func folderPrompt(_ title: String, info: String, text: String, ok: String, then: @escaping (String?) -> Void)
}

// a thread-safe generation counter: a newer scan makes older ones stop
final class FolderGen {
    private var v = 0
    private let lock = NSLock()
    @discardableResult func bump() -> Int { lock.lock(); defer { lock.unlock() }; v += 1; return v }
    var value: Int { lock.lock(); defer { lock.unlock() }; return v }
}

final class FolderSession {
    let leftRoot: String
    let rightRoot: String
    var tree: FolderTree?
    var view = FolderTree.View()
    var rows: [FolderTree.Row] = []
    var cursor = 0
    var anchor: Int?
    var marked = Set<Int>()                  // node ids
    var focus: CompareSide = .left
    var scanning = false
    var scanned = 0
    var checking = false
    var checkDone = 0, checkTotal = 0
    var status = ""
    let gen = FolderGen()
    var hidden = true
    var extraExclude: [String] = []
    var scrollY: CGFloat = 0
    var contentCache: [String: FolderContent.Answer] = [:]
    var scanMs = 0.0
    // kept across a rescan (an operation, F5)
    var expandedKeys = Set<String>()
    var cursorKey: String?
    // this session's own Cmd+Z (never the file browser's ops), kept across
    // new roots; Back / Forward = the root pairs visited (Set as Base Folder,
    // Up One Level)
    var undo = FileOps.UndoStack()
    // git difftool -d: its left copies are written at run time, so times say
    // nothing — every same-size pair gets a content check
    var alwaysContent = false
    var back: [(String, String)] = []
    var forward: [(String, String)] = []

    init(left: String, right: String) { leftRoot = left; rightRoot = right }

    var label: String {
        let l = (leftRoot as NSString).lastPathComponent, r = (rightRoot as NSString).lastPathComponent
        return (l == r ? l : "\(l) ⇆ \(r)") + "/"
    }
}

// MARK: - the drawn tree

final class FolderTreeView: NSView, NSDraggingSource {
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        FileDrag.sourceMask(context)
    }
    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        dragSide = nil
    }

    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    var session: FolderSession? { didSet { needsDisplay = true } }
    var font = NSFont.systemFont(ofSize: 12.5)
    let rowH: CGFloat = 22
    let gutterW: CGFloat = 40
    let sizeW: CGFloat = 70, dateW: CGFloat = 112
    var onClick: ((Int, CompareSide, NSEvent) -> Void)?
    var onDouble: ((Int, CompareSide) -> Void)?
    var onDisclosure: ((Int, Bool) -> Void)?       // row, option held
    var menuFor: ((Int, CompareSide) -> NSMenu?)?
    var onDragOut: ((Int, CompareSide, NSEvent) -> Void)?             // a drag started on a row
    // a drop: the side + row under it, the files, from this tree (= drag across), move
    var onDrop: ((CompareSide, Int?, [URL], Bool, Bool) -> Bool)?
    private var downAt: (row: Int, side: CompareSide, p: NSPoint)?
    fileprivate(set) var dragSide: CompareSide?          // our own drag in flight: from this side
    var dropSide: CompareSide? { didSet { if dropSide != oldValue { needsDisplay = true } } }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    var paneW: CGFloat { max(100, (bounds.width - gutterW) / 2) }
    func sideX(_ s: CompareSide) -> CGFloat { s == .left ? 0 : paneW + gutterW }
    func sideAt(_ x: CGFloat) -> CompareSide { x < paneW + gutterW / 2 ? .left : .right }
    var docHeight: CGFloat { CGFloat(session?.rows.count ?? 0) * rowH + 4 }

    private static let dateFmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd HH:mm"
        return f
    }()

    static func size(_ b: Int64) -> String {
        if b < 1024 { return "\(b) B" }
        let u = ["KB", "MB", "GB", "TB"]
        var v = Double(b) / 1024
        var i = 0
        while v >= 1024 && i < u.count - 1 { v /= 1024; i += 1 }
        return String(format: v < 10 ? "%.1f %@" : "%.0f %@", v, u[i])
    }

    func color(_ n: FolderNode, _ side: CompareSide) -> NSColor {
        switch n.status {
        case .same: return colors.text
        case .different:
            // the older side of a different pair is dimmed (Beyond Compare's gray)
            if n.newer != .none, !n.isDir, (n.newer == .left) != (side == .left) { return colors.dim }
            return colors.tone(.danger)
        case .unimportant: return colors.tone(.info)
        case .leftOnly, .rightOnly: return colors.tone(.accent2)
        case .unknown: return colors.dim
        case .error: return colors.tone(.warning)
        }
    }

    static func glyph(_ n: FolderNode) -> String {
        switch n.status {
        case .same: return "="
        case .different: return n.isDir && !n.kindMismatch ? "≠" : n.newer == .left ? ">" : n.newer == .right ? "<" : "≠"
        case .unimportant: return "≈"
        case .leftOnly, .rightOnly: return ""
        case .unknown: return "…"
        case .error: return "!"
        }
    }

    override func draw(_ dirty: NSRect) {
        colors.base.setFill()
        dirty.fill()
        guard let s = session else { return }
        let first = max(0, Int(dirty.minY / rowH)), last = min(s.rows.count - 1, Int(dirty.maxY / rowH))
        guard first <= last else { return }
        let small = NSFont.systemFont(ofSize: 11.5)
        let p = NSMutableParagraphStyle()
        p.lineBreakMode = .byTruncatingMiddle
        let tree = s.tree
        // the center divider
        colors.hairline.setFill()
        NSRect(x: paneW + gutterW / 2 - 0.5, y: dirty.minY, width: 1, height: dirty.height).fill()
        for i in first...last {
            let row = s.rows[i]
            let n = row.node
            let r = NSRect(x: 0, y: CGFloat(i) * rowH, width: bounds.width, height: rowH)
            let isCursor = i == s.cursor
            let marked = s.marked.contains(n.id)
            // the opaque color under the row's text: names, dates and the
            // glyph are lifted to contrast with it (4.5:1) in every preset
            var under = colors.base
            if marked { under = colors.over(colors.highlight, 0.35, on: under) }
            if isCursor { under = colors.over(colors.highlight, 0.6, on: under) }
            if marked {
                colors.highlight.withAlphaComponent(0.35).setFill()
                r.fill()
            }
            if isCursor {
                let pill = NSBezierPath(roundedRect: r.insetBy(dx: 2, dy: 1), xRadius: 5, yRadius: 5)
                colors.highlight.withAlphaComponent(0.6).setFill()
                pill.fill()
                colors.accentOn.setFill()
                NSRect(x: r.minX + 2, y: r.minY + 4, width: 3, height: r.height - 8).fill()
            }
            for side in [CompareSide.left, .right] {
                let info = side == .left ? n.left : n.right
                let x0 = sideX(side)
                guard let info else {
                    // filler: this side has no such file
                    colors.mantle.withAlphaComponent(0.55).setFill()
                    NSRect(x: x0, y: r.minY + 1, width: paneW, height: rowH - 2).fill()
                    continue
                }
                let col = colors.ensure(color(n, side), on: under)
                var x = x0 + 8 + CGFloat(row.depth) * 16
                if n.isDir && !n.kindMismatch {
                    if !s.view.flatten {
                        let open = n.expanded || (s.view.filter != .all || !s.view.nameFilter.isEmpty)
                        ("\(open ? "▾" : "▸")" as NSString).draw(at: NSPoint(x: x, y: r.minY + 3),
                                                              withAttributes: [.font: small, .foregroundColor: colors.ensure(colors.dim, on: under, 3)])
                    }
                }
                x += 14
                let sym = info.isDir ? "folder" : info.isLink ? "link" : "doc"
                if let img = NSImage(systemSymbolName: sym, accessibilityDescription: nil)?
                    .withSymbolConfiguration(.init(pointSize: 11.5, weight: .regular)) {
                    let tinted = img.copy() as! NSImage
                    tinted.lockFocus()
                    col.set()
                    NSRect(origin: .zero, size: tinted.size).fill(using: .sourceAtop)
                    tinted.unlockFocus()
                    tinted.draw(in: NSRect(x: x, y: r.minY + (rowH - tinted.size.height) / 2, width: tinted.size.width, height: tinted.size.height))
                    x += 19
                }
                let metaW = info.isDir ? dateW + 8 : sizeW + dateW + 16
                let nameW = max(30, x0 + paneW - metaW - x - 6)
                let shown = s.view.flatten ? n.rel : info.name
                let weight: NSFont.Weight = n.isDir ? .medium : .regular
                (shown as NSString).draw(in: NSRect(x: x, y: r.minY + 3, width: nameW, height: 16),
                                         withAttributes: [.font: NSFont.systemFont(ofSize: 12.5, weight: weight),
                                                          .foregroundColor: col, .paragraphStyle: p])
                var mx = x0 + paneW - 8
                let date = Self.dateFmt.string(from: Date(timeIntervalSince1970: info.mtime)) as NSString
                let dattrs: [NSAttributedString.Key: Any] = [.font: small, .foregroundColor: n.isDir && n.status != .same ? colors.ensure(colors.dim, on: under) : col]
                mx -= dateW
                date.draw(at: NSPoint(x: mx, y: r.minY + 4), withAttributes: dattrs)
                if !info.isDir {
                    let sz = Self.size(info.size) as NSString
                    let w = sz.size(withAttributes: dattrs).width
                    sz.draw(at: NSPoint(x: mx - 8 - w, y: r.minY + 4), withAttributes: dattrs)
                }
            }
            // center glyph
            let g = Self.glyph(n) as NSString
            // a "=" from size + time alone is dim: only read bytes earn the bright one
            // (3:1: a status mark, kept visibly dimmer than the 4.5:1 text)
            let gc = n.status == .same && n.sameByMetadata ? colors.ensure(colors.dim, on: under, 3)
                : colors.ensure(color(n, .left), on: under)
            let ga: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: 13, weight: .bold),
                                                     .foregroundColor: gc]
            let gs = g.size(withAttributes: ga)
            g.draw(at: NSPoint(x: paneW + (gutterW - gs.width) / 2, y: r.minY + (rowH - gs.height) / 2), withAttributes: ga)
            _ = tree
        }
        // a drop target: the pane under the drag gets an accent outline
        if let d = dropSide {
            let r = NSRect(x: sideX(d) + 1, y: visibleRect.minY + 1, width: paneW - 2, height: visibleRect.height - 2)
            colors.accentOn.setStroke()
            let path = NSBezierPath(roundedRect: r, xRadius: 6, yRadius: 6)
            path.lineWidth = 2
            path.stroke()
        }
    }

    private func hit(_ e: NSEvent) -> (row: Int, side: CompareSide, x: CGFloat)? {
        let p = convert(e.locationInWindow, from: nil)
        let i = Int(p.y / rowH)
        guard let s = session, s.rows.indices.contains(i) else { return nil }
        return (i, sideAt(p.x), p.x)
    }

    override func mouseDown(with e: NSEvent) {
        window?.makeFirstResponder(self)
        guard let h = hit(e), let s = session else { return }
        // the disclosure triangle
        let n = s.rows[h.row].node
        let x0 = sideX(h.side) + 8 + CGFloat(s.rows[h.row].depth) * 16
        if n.isDir, !n.kindMismatch, !s.view.flatten, h.x >= x0 - 4, h.x < x0 + 14 {
            onDisclosure?(h.row, e.modifierFlags.contains(.option))
            return
        }
        if e.clickCount == 2 { onDouble?(h.row, h.side); return }
        downAt = (h.row, h.side, convert(e.locationInWindow, from: nil))
        onClick?(h.row, h.side, e)
    }

    override func mouseDragged(with e: NSEvent) {
        guard let d = downAt else { return }
        let p = convert(e.locationInWindow, from: nil)
        guard hypot(p.x - d.p.x, p.y - d.p.y) > 4 else { return }
        downAt = nil
        onDragOut?(d.row, d.side, e)
    }

    override func mouseUp(with e: NSEvent) { downAt = nil }

    // the files of `paths` (one side's) as a Finder-style file drag
    func beginDrag(_ paths: [String], side: CompareSide, row: Int, event: NSEvent) {
        guard let first = paths.first else { return }
        dragSide = side
        let label = paths.count > 1 ? "\((first as NSString).lastPathComponent)  +\(paths.count - 1)" : (first as NSString).lastPathComponent
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: colors.text]
        let w = min(paneW - 20, (label as NSString).size(withAttributes: attrs).width + 44)
        let img = NSImage(size: NSSize(width: w, height: rowH), flipped: true) { r in
            self.colors.highlight.withAlphaComponent(0.85).setFill()
            NSBezierPath(roundedRect: r.insetBy(dx: 1, dy: 1), xRadius: 5, yRadius: 5).fill()
            FileDrag.drawFlipped(NSWorkspace.shared.icon(forFile: first), in: NSRect(x: 8, y: 3, width: 16, height: 16))
            (label as NSString).draw(in: NSRect(x: 30, y: 3, width: w - 36, height: 16), withAttributes: attrs)
            return true
        }
        let frame = NSRect(x: sideX(side) + 8, y: CGFloat(row) * rowH, width: w, height: rowH)
        FileDrag.begin(path: first, more: Array(paths.dropFirst()), image: img, frame: frame, view: self, event: event, source: self)
    }

    // MARK: drop target (drag across = copy to the other side; files from Finder)

    private func urls(_ info: NSDraggingInfo) -> [URL] {
        (info.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
    }

    private func operation(_ info: NSDraggingInfo) -> NSDragOperation {
        let side = sideAt(convert(info.draggingLocation, from: nil).x)
        let own = (info.draggingSource as? FolderTreeView) === self
        if own && side == dragSide { dropSide = nil; return [] }
        dropSide = side
        // Cmd = move (the source then offers only .move), else copy
        return info.draggingSourceOperationMask == .move ? .move : .copy
    }

    override func draggingEntered(_ info: NSDraggingInfo) -> NSDragOperation { urls(info).isEmpty ? [] : operation(info) }
    override func draggingUpdated(_ info: NSDraggingInfo) -> NSDragOperation { urls(info).isEmpty ? [] : operation(info) }
    override func draggingExited(_ info: NSDraggingInfo?) { dropSide = nil }
    override func draggingEnded(_ info: NSDraggingInfo) { dropSide = nil }

    override func performDragOperation(_ info: NSDraggingInfo) -> Bool {
        let p = convert(info.draggingLocation, from: nil)
        let side = sideAt(p.x)
        dropSide = nil
        let own = (info.draggingSource as? FolderTreeView) === self
        if own && side == dragSide { return false }
        let i = Int(p.y / rowH)
        let row = (session?.rows.indices.contains(i) ?? false) ? i : nil
        return onDrop?(side, row, urls(info), own, info.draggingSourceOperationMask == .move) ?? false
    }

    override func menu(for e: NSEvent) -> NSMenu? {
        guard let h = hit(e) else { return nil }
        return menuFor?(h.row, h.side)
    }

    override func scrollWheel(with event: NSEvent) { nextResponder?.scrollWheel(with: event) }
}

// MARK: - the page

final class FolderPage: NSObject, NSTextFieldDelegate, QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    weak var host: FolderHost?
    let root = ConfPane()
    private let toolbar = ConfPane()
    private let filterSeg = ConfSegmented(FolderFilter.allCases.map(\.title))
    private let summary = NSTextField(labelWithString: "")
    private let nameBox = JiraInputBox(placeholder: "names: *.swift, !*.o")
    private var buttons: [ThemeButton] = []
    private let headerL = CompareHeaderLabel(), headerR = CompareHeaderLabel()
    let tree = FolderTreeView()
    private let scroll = NSScrollView()
    private let status = NSTextField(labelWithString: "")
    private let emptyHint = NSTextField(labelWithString: "")
    private(set) var session: FolderSession?
    private var colors: PopupColors { host?.folderColors ?? PopupThemeDefaults.colors }
    private var options: FolderOptions { makeOptions() }
    private let queue = DispatchQueue(label: "folder-compare", qos: .userInitiated)
    private let checkQueue = DispatchQueue(label: "folder-compare-content", qos: .utility)
    private var flatten: ThemeButton!
    var testClash: FileOps.Clash?            // a test pre-answers the clash sheet
    private var quickLookPaths: [String] = []
    private(set) var lastPlan: (mode: SyncMode, plan: SyncPlan)?
    private var syncButton: ThemeButton!

    init(host: FolderHost) {
        self.host = host
        super.init()
        build()
    }

    private func label(_ f: NSTextField, size: CGFloat, weight: NSFont.Weight = .regular, color: NSColor? = nil) {
        f.font = .systemFont(ofSize: size, weight: weight)
        f.textColor = color ?? colors.dim
        f.lineBreakMode = .byTruncatingTail
        f.isSelectable = false
    }

    private func button(_ symbol: String, _ tip: String, _ action: @escaping () -> Void) -> ThemeButton {
        var bc = PopupConfig(name: "compare-tools")
        bc.colors = colors
        let b = ThemeButton(config: bc, title: "", symbol: symbol)
        b.flat = true
        b.toolTip = tip
        b.onClick = action
        return b
    }

    private func build() {
        toolbar.fill = colors.mantle
        filterSeg.colors = colors
        filterSeg.tips = ["Everything (⌘1)", "Only differences (⌘2)", "Only identical items (⌘3)", "Only items on one side (⌘4)",
                          "Left side newer (⌘5)", "Right side newer (⌘6)"]
        filterSeg.onChange = { [weak self] i in self?.setFilter(FolderFilter.allCases[i]) }
        toolbar.addSubview(filterSeg)
        label(summary, size: 12, weight: .semibold, color: colors.text)
        toolbar.addSubview(summary)
        flatten = button("list.bullet.indent", "Ignore folder structure: list every file by its path") { [weak self] in self?.toggleFlatten() }
        // Synchronize can trash files: a WORDED raised button set apart from the
        // icon row (its circular arrows read as Rescan's when it was one of them)
        syncButton = button("arrow.triangle.2.circlepath", "Synchronize… (Update / Mirror, with a preview) — ⌘K") { [weak self] in self?.showSyncMenu() }
        syncButton.title = (host?.folderConfig ?? CompareConfig()).label("sync-button", "Sync…")
        syncButton.flat = false
        toolbar.addSubview(syncButton)
        buttons = [
            button("chevron.up", "Previous difference (⌃P)") { [weak self] in self?.jumpDiff(-1) },
            button("chevron.down", "Next difference (⌃N)") { [weak self] in self?.jumpDiff(1) },
            button("arrow.right.to.line", "Copy the selection to the right (⌥→)") { [weak self] in self?.copy(from: .left) },
            button("arrow.left.to.line", "Copy the selection to the left (⌥←)") { [weak self] in self?.copy(from: .right) },
            flatten,
            button("arrow.up.and.down.text.horizontal", "Expand / collapse all (⌥→ / ⌥←)") { [weak self] in self?.toggleAll() },
            button("chevron.backward", "Back (⌘[)") { [weak self] in self?.goBack() },
            button("arrow.up", "Up one level, both sides (⌘↑)") { [weak self] in self?.upOneLevel() },
            button("arrow.left.arrow.right", "Swap sides (⌘⌥X)") { [weak self] in self?.swapSides() },
            button("arrow.clockwise", "Scan again (F5)") { [weak self] in self?.rescan() },
        ]
        buttons.forEach(toolbar.addSubview)
        nameBox.field.delegate = self
        toolbar.addSubview(nameBox)
        root.addSubview(toolbar)
        for (h, side) in [(headerL, CompareSide.left), (headerR, .right)] {
            h.colors = colors
            h.onClick = { [weak self] in self?.session?.focus = side; self?.sync() }
            root.addSubview(h)
        }
        tree.colors = colors
        tree.onClick = { [weak self] row, side, e in self?.clicked(row, side, e) }
        tree.onDouble = { [weak self] row, side in self?.session?.focus = side; self?.activate(row) }
        tree.onDisclosure = { [weak self] row, opt in self?.toggleExpand(row, recursive: opt) }
        tree.menuFor = { [weak self] row, side in self?.menu(row, side) }
        tree.onDragOut = { [weak self] row, side, e in self?.dragOut(row, side, e) }
        tree.onDrop = { [weak self] side, row, urls, own, move in self?.drop(side, row, urls, own: own, move: move) ?? false }
        tree.registerForDraggedTypes([.fileURL])
        scroll.documentView = tree
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        root.addSubview(scroll)
        label(emptyHint, size: 12.5)
        emptyHint.alignment = .center
        emptyHint.isHidden = true
        root.addSubview(emptyHint)
        label(status, size: 11.5)
        status.lineBreakMode = .byTruncatingMiddle
        root.addSubview(status)
        root.onLayout = { [weak self] b in self?.layout(b) }
    }

    private func layout(_ b: NSRect) {
        let tbH: CGFloat = 36, hdrH: CGFloat = 26, statusH: CGFloat = 24
        toolbar.frame = NSRect(x: 0, y: 0, width: b.width, height: tbH)
        let segW = filterSeg.intrinsicContentSize.width
        filterSeg.frame = NSRect(x: 12, y: (tbH - JiraTheme.height) / 2, width: segW, height: JiraTheme.height)
        var bx = b.width - 10
        for btn in buttons.reversed() {
            bx -= 28
            btn.frame = NSRect(x: bx, y: (tbH - 26) / 2, width: 26, height: 26)
            bx -= 2
        }
        let sw = syncButton.fittingWidth()
        bx -= 10
        syncButton.frame = NSRect(x: bx - sw, y: (tbH - 26) / 2, width: sw, height: 26)
        bx -= sw
        let nw: CGFloat = min(220, max(120, (b.width - segW - 28 * CGFloat(buttons.count) - sw) * 0.3))
        nameBox.frame = NSRect(x: bx - nw - 6, y: (tbH - JiraTheme.height) / 2, width: nw, height: JiraTheme.height)
        summary.frame = NSRect(x: filterSeg.frame.maxX + 14, y: (tbH - 18) / 2,
                               width: max(40, nameBox.frame.minX - filterSeg.frame.maxX - 24), height: 18)
        let pw = max(100, (b.width - tree.gutterW) / 2)
        headerL.frame = NSRect(x: 0, y: tbH, width: pw, height: hdrH)
        headerR.frame = NSRect(x: pw + tree.gutterW, y: tbH, width: pw, height: hdrH)
        let top = tbH + hdrH
        scroll.frame = NSRect(x: 0, y: top, width: b.width, height: max(40, b.height - top - statusH))
        emptyHint.frame = NSRect(x: 0, y: top + 40, width: b.width, height: 20)
        status.frame = NSRect(x: 12, y: b.height - statusH + 4, width: b.width - 24, height: 16)
        resizeTree()
    }

    private func resizeTree() {
        let w = scroll.contentSize.width
        let h = max(scroll.contentSize.height, tree.docHeight)
        if tree.frame.size != NSSize(width: w, height: h) { tree.frame = NSRect(x: 0, y: 0, width: w, height: h) }
    }

    // MARK: options (config) and scan

    private func makeOptions() -> FolderOptions {
        let c = host?.folderConfig ?? CompareConfig()
        var o = FolderOptions()
        o.timeTolerance = max(0, c.number("time-tolerance", 2))
        o.content = ["auto", "always", "never"].contains(c.string("content", "auto")) ? c.string("content", "auto") : "auto"
        if session?.alwaysContent == true { o.content = "always" }
        o.hidden = session?.hidden ?? true
        o.exclude = c.string("exclude", ".git, node_modules, .DS_Store").split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty } + (session?.extraExclude ?? [])
        o.importance = c.importance
        if c.bool("use-gitignore", true) {
            let rules = IgnoreRules()
            let file = (c.string("ignore-file", "") as NSString).expandingTildeInPath
            if !file.isEmpty, FileManager.default.fileExists(atPath: file) { rules.shelfFile = file }
            rules.recheck = 30
            o.ignored = { rules.ignored($0, isDir: $1) }
        }
        return o
    }

    func bind(_ s: FolderSession) {
        session = s
        tree.session = s
        headerL.text = tilde(s.leftRoot)
        headerR.text = tilde(s.rightRoot)
        if s.tree == nil && !s.scanning { rescan() } else { sync() }
    }

    private func tilde(_ p: String) -> String { (p as NSString).abbreviatingWithTildeInPath }

    // the tree again (first open, F5, after an operation): expansion + cursor survive
    func rescan(keepStatus: Bool = false) {
        guard let s = session else { return }
        if let t = s.tree {
            s.expandedKeys = Set(t.all.filter { $0.expanded }.map(\.key))
            if s.rows.indices.contains(s.cursor) { s.cursorKey = s.rows[s.cursor].node.key }
        }
        let gen = s.gen.bump()
        s.scanning = true
        s.scanned = 0
        s.checking = false
        if !keepStatus { s.status = "" }
        let o = makeOptions()
        let (l, r) = (s.leftRoot, s.rightRoot)
        let t0 = DispatchTime.now().uptimeNanoseconds
        sync()
        queue.async { [weak self, weak s] in
            let tree = FolderScan.run(left: l, right: r, options: o,
                                      progress: { n in DispatchQueue.main.async { if let s, s.gen.value == gen { s.scanned = n; self?.sync() } } },
                                      cancelled: { s?.gen.value != gen })
            DispatchQueue.main.async {
                guard let self, let s, s.gen.value == gen else { return }
                s.scanMs = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
                self.scanned(tree, s, o, gen)
            }
        }
    }

    private func scanned(_ tree: FolderTree, _ s: FolderSession, _ o: FolderOptions, _ gen: Int) {
        // answers remembered from an earlier scan (an unchanged pair)
        for n in tree.pending {
            if let a = s.contentCache[cacheKey(tree, n)] { FolderContent.apply(a, to: n) }
        }
        for n in tree.all where s.expandedKeys.contains(n.key) { n.expanded = true }
        if s.tree == nil, tree.roots.count < 40 { for n in tree.roots where n.isDir { n.expanded = true } }
        tree.settle()
        s.tree = tree
        s.scanning = false
        rebuild(s)
        host?.folderLog(String(format: "compare folder: %d items, scan %.0f ms (%@ ⇆ %@)", tree.all.count, s.scanMs,
                               (s.leftRoot as NSString).lastPathComponent, (s.rightRoot as NSString).lastPathComponent))
        // the pairs the quick test left open + the rules pass on different ones
        let pend = tree.pending
        let cands = o.content == "never" ? [] : FolderContent.ruleCandidates(tree)
        guard !pend.isEmpty || !cands.isEmpty else { sync(); return }
        s.checking = true
        s.checkDone = 0
        s.checkTotal = pend.count + cands.count
        sync()
        runChecks(tree, s, pend, o, gen, rules: false) { [weak self] in
            self?.runChecks(tree, s, cands, o, gen, rules: true) {
                s.checking = false
                self?.rebuild(s)
                self?.host?.folderLog("compare folder: content checked (\(pend.count) pairs, \(cands.count) rule checks)")
            }
        }
    }

    private func cacheKey(_ t: FolderTree, _ n: FolderNode) -> String {
        guard let l = n.left, let r = n.right else { return n.key }
        return "\(t.path(n, .left))|\(l.size)|\(l.mtime)|\(t.path(n, .right))|\(r.size)|\(r.mtime)"
    }

    private func runChecks(_ tree: FolderTree, _ s: FolderSession, _ nodes: [FolderNode], _ o: FolderOptions, _ gen: Int,
                           rules: Bool, then: @escaping () -> Void) {
        guard !nodes.isEmpty else { then(); return }
        let imp = o.importance
        FolderContent.run(tree: tree, nodes: nodes, imp: imp, queue: checkQueue,
                          cancelled: { s.gen.value != gen },
                          batch: { [weak self] batch in
                              DispatchQueue.main.async {
                                  guard let self, s.gen.value == gen else { return }
                                  for (id, a) in batch {
                                      guard let n = tree.node(id) else { continue }
                                      // the rules pass only ever softens "different" to "unimportant"
                                      if rules && a != .unimportant { s.checkDone += 1; continue }
                                      FolderContent.apply(a, to: n)
                                      s.contentCache[self.cacheKey(tree, n)] = a
                                      s.checkDone += 1
                                  }
                                  tree.settle()
                                  self.rebuild(s)
                              }
                          },
                          done: { DispatchQueue.main.async { if s.gen.value == gen { then() } } })
    }

    // MARK: rows

    func rebuild(_ s: FolderSession) {
        guard let t = s.tree else { sync(); return }
        let keep = s.rows.indices.contains(s.cursor) ? s.rows[s.cursor].node.key : s.cursorKey
        s.rows = t.rows(s.view)
        if let k = keep, let i = s.rows.firstIndex(where: { $0.node.key == k }) { s.cursor = i } else { s.cursor = min(s.cursor, max(0, s.rows.count - 1)) }
        s.cursorKey = nil
        s.marked = s.marked.filter { id in s.rows.contains { $0.node.id == id } }
        sync()
    }

    func sync() {
        guard let s = session else { return }
        filterSeg.selected = FolderFilter.allCases.firstIndex(of: s.view.filter) ?? 0
        flatten.isOn = s.view.flatten
        headerL.focused = s.focus == .left
        headerR.focused = s.focus == .right
        headerL.text = tilde(s.leftRoot)
        headerR.text = tilde(s.rightRoot)
        summary.stringValue = summaryText(s)
        summary.textColor = s.tree.map { let c = $0.counts()
            return c.different + c.leftOnly + c.rightOnly + c.sameByMetadata == 0 && !s.scanning && !s.checking } == true
            ? colors.tone(.success) : colors.text
        status.stringValue = statusText(s)
        emptyHint.stringValue = s.scanning ? "scanning…" : s.tree != nil && s.rows.isEmpty
            ? (s.view.filter == .all && s.view.nameFilter.isEmpty ? "Both folders are empty" : "Nothing matches this filter") : ""
        emptyHint.isHidden = emptyHint.stringValue.isEmpty
        resizeTree()
        tree.needsDisplay = true
        host?.folderChanged()
    }

    private func summaryText(_ s: FolderSession) -> String {
        guard let t = s.tree else { return s.scanning ? "scanning… \(s.scanned) items" : "" }
        let c = t.counts()
        var parts: [String] = []
        if c.different > 0 { parts.append("\(c.different) differ") }
        if c.unimportant > 0 { parts.append("\(c.unimportant) unimportant") }
        if c.leftOnly > 0 { parts.append("\(c.leftOnly) left only") }
        if c.rightOnly > 0 { parts.append("\(c.rightOnly) right only") }
        if c.unknown > 0 { parts.append("\(c.unknown) unchecked") }
        let byDate = c.sameByMetadata > 0 ? " (" + byDateLabel(c.sameByMetadata) + ")" : ""
        if c.same > 0 { parts.append("\(c.same) same" + byDate) }
        if c.error > 0 { parts.append("\(c.error) unreadable") }
        if parts.isEmpty { return "empty" }
        if c.different + c.leftOnly + c.rightOnly + c.unknown + c.unimportant == 0 { return "Identical — \(c.same) files" + byDate }
        return parts.joined(separator: " · ")
    }

    private func byDateLabel(_ n: Int) -> String {
        (host?.folderConfig ?? CompareConfig()).label("same-by-date", "{} by date/size only", String(n))
    }

    private func statusText(_ s: FolderSession) -> String {
        var bits: [String] = []
        if s.scanning { bits.append("scanning \(s.scanned) items…") }
        if s.checking { bits.append("checking content \(s.checkDone)/\(s.checkTotal)") }
        if let t = s.tree {
            if t.truncated { bits.append("stopped at 400000 items") }
            if !t.errors.isEmpty { bits.append("\(t.errors.count) folder(s) unreadable") }
            if !s.view.nameFilter.isEmpty { bits.append("names: \(s.view.nameFilter)") }
            if s.view.flatten { bits.append("flat") }
            if t.counts().sameByMetadata > 0 {
                bits.append((host?.folderConfig ?? CompareConfig()).label(
                    "same-by-date-hint", "dim = : same size + time, bytes not read ([compare] content = always reads them)"))
            }
        }
        if !s.status.isEmpty { bits.insert(s.status, at: 0) }
        return bits.joined(separator: "  ·  ")
    }

    // MARK: view state

    func setFilter(_ f: FolderFilter) {
        guard let s = session else { return }
        s.view.filter = f
        rebuild(s)
    }

    func toggleFlatten() {
        guard let s = session else { return }
        s.view.flatten.toggle()
        rebuild(s)
    }

    private func toggleAll() {
        guard let s = session, let t = s.tree else { return }
        let open = !t.all.contains { $0.isDir && $0.expanded }
        t.expandAll(open)
        rebuild(s)
    }

    func swapSides() {
        guard let s = session else { return }
        // a swapped session is a new pair: roots swap, the scan runs again
        let sw = successor(s, left: s.rightRoot, right: s.leftRoot)
        sw.focus = s.focus.other
        host?.folderSwapped(old: s, new: sw)
    }

    // MARK: base folder, back / forward, up one level

    // a session on new roots that keeps this one's view, rules and undo
    private func successor(_ s: FolderSession, left: String, right: String) -> FolderSession {
        let n = FolderSession(left: left, right: right)
        n.view = s.view
        n.hidden = s.hidden
        n.extraExclude = s.extraExclude
        n.undo = s.undo
        n.alwaysContent = s.alwaysContent
        n.focus = s.focus
        return n
    }

    // new roots, remembered for Back
    func rebase(left: String, right: String) {
        guard let s = session, left != s.leftRoot || right != s.rightRoot else { return }
        let n = successor(s, left: left, right: right)
        n.back = s.back + [(s.leftRoot, s.rightRoot)]
        host?.folderSwapped(old: s, new: n)
    }

    func goBack() {
        guard let s = session else { return }
        guard let (l, r) = s.back.last else { s.status = "nothing to go back to"; sync(); return }
        let n = successor(s, left: l, right: r)
        n.back = Array(s.back.dropLast())
        n.forward = s.forward + [(s.leftRoot, s.rightRoot)]
        host?.folderSwapped(old: s, new: n)
    }

    func goForward() {
        guard let s = session else { return }
        guard let (l, r) = s.forward.last else { s.status = "nothing to go forward to"; sync(); return }
        let n = successor(s, left: l, right: r)
        n.back = s.back + [(s.leftRoot, s.rightRoot)]
        n.forward = Array(s.forward.dropLast())
        host?.folderSwapped(old: s, new: n)
    }

    // both roots one level up (Cmd+Up)
    func upOneLevel() {
        guard let s = session else { return }
        let l = (s.leftRoot as NSString).deletingLastPathComponent, r = (s.rightRoot as NSString).deletingLastPathComponent
        guard l != s.leftRoot, r != s.rightRoot, !l.isEmpty, !r.isEmpty else { s.status = "already at the top"; sync(); return }
        rebase(left: l, right: r)
    }

    // a folder row as the new root: both sides (a folder pair), or one side
    // (the other root stays)
    func setBase(_ row: Int, side: CompareSide?) {
        guard let s = session, let t = s.tree, let n = node(at: row) else { return }
        func isFolder(_ i: FolderSideInfo?) -> Bool { i?.isDir == true }
        switch side {
        case nil:
            guard isFolder(n.left), isFolder(n.right) else { s.status = "Set as Base Folder needs a folder on both sides"; sync(); return }
            rebase(left: t.path(n, .left), right: t.path(n, .right))
        case .left?:
            guard isFolder(n.left) else { return }
            rebase(left: t.path(n, .left), right: s.rightRoot)
        case .right?:
            guard isFolder(n.right) else { return }
            rebase(left: s.leftRoot, right: t.path(n, .right))
        }
    }

    func setNameFilter(_ text: String) {
        guard let s = session else { return }
        s.view.nameFilter = text
        rebuild(s)
    }

    // the name box types live
    func controlTextDidChange(_ obj: Notification) {
        guard (obj.object as? NSTextField) === nameBox.field else { return }
        setNameFilter(nameBox.field.stringValue)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        if sel == #selector(NSResponder.insertNewline(_:)) || sel == #selector(NSResponder.moveDown(_:)) || sel == #selector(NSResponder.insertTab(_:)) {
            focusTree()
            return true
        }
        return false
    }

    func focusTree() { host?.folderWindow.makeFirstResponder(tree) }
    func focusNameBox() { host?.folderWindow.makeFirstResponder(nameBox.field) }
    var nameBoxFocused: Bool { host?.folderWindow.firstResponder === nameBox.field.currentEditor() }

    // MARK: cursor + selection

    private func node(at row: Int) -> FolderNode? {
        guard let s = session, s.rows.indices.contains(row) else { return nil }
        return s.rows[row].node
    }

    func move(to row: Int, extend: Bool = false) {
        guard let s = session, !s.rows.isEmpty else { return }
        let r = max(0, min(s.rows.count - 1, row))
        if extend {
            if s.anchor == nil { s.anchor = s.cursor }
            let lo = min(s.anchor!, r), hi = max(s.anchor!, r)
            s.marked = Set((lo...hi).map { s.rows[$0].node.id })
        } else {
            s.anchor = nil
            s.marked = []
        }
        s.cursor = r
        scrollToCursor()
        sync()
    }

    private func scrollToCursor() {
        guard let s = session else { return }
        let r = NSRect(x: 0, y: CGFloat(s.cursor) * tree.rowH, width: 1, height: tree.rowH)
        tree.scrollToVisible(r.insetBy(dx: 0, dy: -tree.rowH))
    }

    private func clicked(_ row: Int, _ side: CompareSide, _ e: NSEvent) {
        guard let s = session, s.rows.indices.contains(row) else { return }
        s.focus = side
        let id = s.rows[row].node.id
        if e.modifierFlags.contains(.command) {
            if s.marked.isEmpty, s.rows.indices.contains(s.cursor) { s.marked.insert(s.rows[s.cursor].node.id) }
            if s.marked.contains(id) { s.marked.remove(id) } else { s.marked.insert(id) }
            s.cursor = row
            s.anchor = row
            sync()
        } else if e.modifierFlags.contains(.shift) {
            move(to: row, extend: true)
        } else {
            s.cursor = row
            s.anchor = nil
            s.marked = []
            sync()
        }
    }

    // the nodes an action works on: the marked rows, else the cursor row;
    // a folder already covered by a marked parent is dropped
    func targets() -> [FolderNode] {
        guard let s = session else { return [] }
        var ns: [FolderNode]
        if !s.marked.isEmpty { ns = s.rows.map(\.node).filter { s.marked.contains($0.id) } }
        else if let n = node(at: s.cursor) { ns = [n] } else { ns = [] }
        let keys = Set(ns.filter { $0.isDir }.map(\.key))
        ns = ns.filter { n in
            var p = n.parent
            while let q = p { if keys.contains(q.key) { return false }; p = q.parent }
            return true
        }
        return ns
    }

    // MARK: expand / open

    func toggleExpand(_ row: Int, recursive: Bool = false, to: Bool? = nil) {
        guard let s = session, let n = node(at: row), n.isDir, !n.kindMismatch, !s.view.flatten else { return }
        let open = to ?? !n.expanded
        func set(_ x: FolderNode) {
            x.expanded = open
            if recursive { x.children.filter { $0.isDir }.forEach(set) }
        }
        set(n)
        rebuild(s)
    }

    func activate(_ row: Int) {
        guard let s = session, let t = s.tree, let n = node(at: row) else { return }
        if n.isDir && !n.kindMismatch { toggleExpand(row); return }
        if n.left?.isDir == true || n.right?.isDir == true { return }
        host?.folderOpenPair(n.left != nil ? t.path(n, .left) : nil, n.right != nil ? t.path(n, .right) : nil)
    }

    func jumpDiff(_ dir: Int) {
        guard let s = session, !s.rows.isEmpty else { return }
        let isDiffRow: (FolderTree.Row) -> Bool = { !($0.node.isDir && !$0.node.kindMismatch) && $0.node.status.isDiff }
        var i = s.cursor + dir
        while s.rows.indices.contains(i) {
            if isDiffRow(s.rows[i]) { move(to: i); return }
            i += dir
        }
        s.status = "no more differences"
        sync()
    }

    // MARK: keys (the window routes them here when a folder session is up)

    func handleKey(_ e: NSEvent) -> Bool {
        guard let s = session else { return false }
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        let opt = mods.contains(.option), shift = mods.contains(.shift)
        let code = e.keyCode
        let inText = host?.folderWindow.firstResponder is NSText
        if inText {
            if cmd && code == 3 { focusNameBox(); return true }
            return false                                                         // the name box + edit keys
        }
        let page = max(1, Int(scroll.contentSize.height / tree.rowH) - 2)
        switch code {
        case 45 where ctrl && !cmd: jumpDiff(1)                                  // Ctrl+N
        case 35 where ctrl && !cmd: jumpDiff(-1)                                 // Ctrl+P
        // copy across: Opt+→ / Opt+← (Ctrl+Opt = move), Ctrl+R too; plain
        // Ctrl+L is the pane move now (SharedWindow)
        case 15 where ctrl && !cmd: opt ? transfer(from: .left, move: true) : copy(from: .left)     // Ctrl+R / Ctrl+Opt+R
        case 37 where ctrl && opt && !cmd: transfer(from: .right, move: true)                       // Ctrl+Opt+L
        case 124 where opt && !cmd: ctrl ? transfer(from: .left, move: true) : copy(from: .left)    // Opt+→ / Ctrl+Opt+→
        case 123 where opt && !cmd: ctrl ? transfer(from: .right, move: true) : copy(from: .right)  // Opt+← / Ctrl+Opt+←
        case 6 where cmd && !shift: undo()                                       // Cmd+Z (this session's own ops)
        case 51 where cmd: trash()                                               // Cmd+Delete
        case 18 where cmd, 19 where cmd, 20 where cmd, 21 where cmd, 23 where cmd, 22 where cmd:   // Cmd+1…6
            setFilter(FolderFilter.allCases[[18: 0, 19: 1, 20: 2, 21: 3, 23: 4, 22: 5][Int(code)]!])
        case 3 where cmd: focusNameBox()                                         // Cmd+F
        case 7 where cmd && opt: swapSides()                                     // Cmd+Opt+X
        case 96: rescan()                                                        // F5
        case 47 where cmd && shift:                                              // Cmd+Shift+.
            s.hidden.toggle()
            rescan()
        case 45 where cmd && shift: newFolder()                                  // Cmd+Shift+N
        case 15 where cmd, 120: beginRename()                                    // Cmd+R / F2
        case 8 where cmd || ctrl: copyPaths()                                    // Cmd+C
        case 0 where cmd:                                                        // Cmd+A
            s.marked = Set(s.rows.map(\.node.id))
            sync()
        case 48 where !ctrl && !cmd:                                             // Tab: the other side
            s.focus = s.focus.other
            sync()
        case 126 where !cmd: move(to: s.cursor - 1, extend: shift)               // ↑
        case 125 where !cmd: move(to: s.cursor + 1, extend: shift)               // ↓
        case 126 where cmd: upOneLevel()                                         // Cmd+Up: both up one level
        case 125 where cmd:                                                      // Cmd+Down: into a folder pair
            if let n = node(at: s.cursor), n.isDir, !n.kindMismatch {
                setBase(s.cursor, side: n.left != nil && n.right != nil ? nil : n.left != nil ? .left : .right)
            } else { activate(s.cursor) }
        case 33 where cmd: goBack()                                              // Cmd+[
        case 30 where cmd: goForward()                                           // Cmd+]
        case 115: move(to: 0, extend: shift)                                     // Home
        case 119: move(to: s.rows.count - 1, extend: shift)                      // End
        case 116: move(to: s.cursor - page, extend: shift)
        case 121: move(to: s.cursor + page, extend: shift)
        case 124 where !cmd && !ctrl:                                            // →: expand (Shift = all below)
            if let n = node(at: s.cursor), n.isDir, !n.expanded { toggleExpand(s.cursor, recursive: shift, to: true) }
            else if shift { t_expandAll(true) }
            else { move(to: s.cursor + 1) }
        case 123 where !cmd && !ctrl:                                            // ←: collapse, else up to the parent
            if let n = node(at: s.cursor), n.isDir, n.expanded { toggleExpand(s.cursor, recursive: shift, to: false) }
            else if shift { t_expandAll(false) }
            else if let p = node(at: s.cursor)?.parent, let i = s.rows.firstIndex(where: { $0.node === p }) { move(to: i) }
        case 36, 76: activate(s.cursor)                                          // Return
        case 49 where !cmd && !ctrl && !opt: toggleQuickLook()                   // Space: Quick Look
        case 16 where cmd: toggleQuickLook()                                     // Cmd+Y
        default: return false
        }
        return true
    }

    private func t_expandAll(_ open: Bool) {
        guard let s = session, let t = s.tree else { return }
        t.expandAll(open)
        rebuild(s)
    }

    // Esc: the page's own steps first (name box, marks, a running scan); false = nothing left
    func escape() -> Bool {
        guard let s = session else { return false }
        if nameBoxFocused {
            if !nameBox.field.stringValue.isEmpty { nameBox.field.stringValue = ""; setNameFilter("") }
            focusTree()
            return true
        }
        if s.scanning || s.checking {
            s.gen.bump()
            s.scanning = false
            s.checking = false
            s.status = "stopped"
            sync()
            return true
        }
        if !s.marked.isEmpty || s.anchor != nil { s.marked = []; s.anchor = nil; sync(); return true }
        if !s.view.nameFilter.isEmpty { nameBox.field.stringValue = ""; setNameFilter(""); return true }
        return false
    }

    // MARK: file actions (FileOps: Trash, undo, " 2" names)

    private func report(_ out: FileOps.Outcome) {
        for c in out.changes { FileDrag.onFileOp?(c.from, c.to) }
    }

    private func finish(_ s: FolderSession, _ word: String, _ out: FileOps.Outcome) {
        report(out)
        s.status = out.failed.map { "\(word): \($0)" } ?? word
        rescan(keepStatus: true)
    }

    // what to put where: every selected node that exists on `from`, folders
    // pairs descended into (a merge), identical items skipped
    private func plan(from: CompareSide) -> [(src: String, dst: String, clash: Bool)] {
        guard let s = session, let t = s.tree else { return [] }
        var items: [(String, String, Bool)] = []
        func visit(_ n: FolderNode) {
            let src = from == .left ? n.left : n.right
            let other = from == .left ? n.right : n.left
            guard src != nil else { return }
            if n.isDir, !n.kindMismatch, other != nil {
                n.children.forEach(visit)
                return
            }
            if n.status == .same { return }
            items.append((t.path(n, from), t.path(n, from.other), other != nil))
        }
        targets().forEach(visit)
        return items
    }

    func copy(from: CompareSide) { transfer(from: from, move: false) }

    func transfer(from: CompareSide, move: Bool) {
        guard let s = session, s.tree != nil else { return }
        let items = plan(from: from)
        guard !items.isEmpty else {
            s.status = "nothing to \(move ? "move" : "copy") — the selection is identical or not on the \(from.rawValue) side"
            sync()
            return
        }
        let word = "\(move ? "moved" : "copied") \(items.count) item\(items.count == 1 ? "" : "s") to the \(from.other.rawValue)"
        let clashes = items.filter { $0.clash }.count
        let run: (FileOps.Clash) -> Void = { [weak self] clash in
            s.status = "working…"
            self?.sync()
            DispatchQueue.global(qos: .userInitiated).async {
                let out = FileOps.place(items.map { ($0.src, $0.dst) }, move: move, clash: clash, undo: s.undo)
                DispatchQueue.main.async { self?.finish(s, word, out) }
            }
        }
        if clashes > 0, let c = testClash { run(c); return }
        guard clashes > 0, let host else { run(.replace); return }
        let cfg = host.folderConfig
        // Cancel is the default (Return): Replace has to be picked on purpose
        host.folderConfirm(
            clashes == 1 ? cfg.label("clash-one", "1 item already exists on the {} side", from.other.rawValue)
                         : cfg.label("clash", "{} items already exist on the other side", String(clashes)),
            info: cfg.label("clash-info", "Replace puts the old ones in the Trash (⌘Z brings them back)."),
            accessory: nil,
            choices: [(cfg.label("replace-button", "Replace"), .danger), (cfg.label("keep-both-button", "Keep Both"), .normal),
                      (cfg.label("skip-button", "Skip Existing"), .normal), (cfg.label("cancel-button", "Cancel"), .primary)],
            defaultIndex: 3, cancelIndex: 3) { i in
            switch i {
            case 0: run(.replace)
            case 1: run(.keepBoth)
            case 2: run(.skip)
            default: break
            }
        }
    }

    func trash() {
        guard let s = session, let t = s.tree else { return }
        let paths = targets().filter { ($0.left != nil && s.focus == .left) || ($0.right != nil && s.focus == .right) }
            .map { t.path($0, s.focus) }
        guard !paths.isEmpty else { s.status = "nothing on the \(s.focus.rawValue) side to trash"; sync(); return }
        let out = FileOps.trash(paths, undo: s.undo)
        finish(s, "moved \(out.changes.count) item\(out.changes.count == 1 ? "" : "s") to the Trash (⌘Z undoes)", out)
    }

    func undo() {
        guard let s = session else { return }
        guard let (what, out) = FileOps.undo(s.undo) else { s.status = "nothing to undo in this session"; sync(); return }
        finish(s, "undid \(what)", out)
    }

    func beginRename() {
        guard let s = session, let t = s.tree, let host, let n = targets().first,
              (s.focus == .left ? n.left : n.right) != nil else { return }
        let path = t.path(n, s.focus)
        host.folderPrompt("Rename", info: tilde(path), text: (path as NSString).lastPathComponent, ok: "Rename") { [weak self] answer in
            let name = (answer ?? "").trimmingCharacters(in: .whitespaces)
            guard answer != nil, !name.isEmpty, !name.contains("/"), name != (path as NSString).lastPathComponent else { return }
            let to = ((path as NSString).deletingLastPathComponent as NSString).appendingPathComponent(name)
            do {
                try FileManager.default.moveItem(atPath: path, toPath: to)
                FileOps.recordRename(from: path, to: to, undo: s.undo)
                FileDrag.onFileOp?(path, to)
                self?.session.map { self?.finish($0, "renamed to \(name)", FileOps.Outcome()) }
            } catch {
                s.status = "rename: \(error.localizedDescription)"
                self?.sync()
            }
        }
    }

    func newFolder() {
        guard let s = session, let t = s.tree, let host else { return }
        let n = targets().first
        let base: String
        if let n, n.isDir, (s.focus == .left ? n.left : n.right) != nil { base = t.path(n, s.focus) }
        else if let n { base = (t.path(n, s.focus) as NSString).deletingLastPathComponent }
        else { base = s.focus == .left ? s.leftRoot : s.rightRoot }
        host.folderPrompt("New Folder", info: "in \(tilde(base))", text: "untitled folder", ok: "Create") { [weak self] answer in
            let name = (answer ?? "").trimmingCharacters(in: .whitespaces)
            guard answer != nil, !name.isEmpty, !name.contains("/") else { return }
            let out = FileOps.create(name, in: base, folder: true, undo: s.undo)
            self?.finish(s, "created \(name)", out)
        }
    }

    func copyPaths() {
        guard let s = session, let t = s.tree else { return }
        let paths = targets().filter { (s.focus == .left ? $0.left : $0.right) != nil }.map { t.path($0, s.focus) }
        guard !paths.isEmpty else { return }
        copyText(paths.joined(separator: "\n"))
        s.status = "copied \(paths.count) path\(paths.count == 1 ? "" : "s")"
        sync()
    }

    // MARK: right-click (rule 2)

    private func menu(_ row: Int, _ side: CompareSide) -> NSMenu? {
        guard let s = session, let t = s.tree, let n = node(at: row) else { return nil }
        s.focus = side
        if !s.marked.contains(n.id) { s.marked = []; s.cursor = row }
        sync()
        let m = NSMenu()
        let isFolder = n.isDir && !n.kindMismatch
        let onSide = (side == .left ? n.left : n.right) != nil
        let path = onSide ? t.path(n, side) : nil
        if isFolder {
            m.addItem(menuItem(n.expanded ? "Collapse" : "Expand") { [weak self] in self?.toggleExpand(row) })
        } else {
            m.addItem(menuItem(n.left != nil && n.right != nil ? "Open (Compare)" : "Open in Text Compare") { [weak self] in self?.activate(row) })
        }
        m.addItem(.separator())
        m.addItem(menuItem("Copy to Right  ⌥→", enabled: n.left != nil) { [weak self] in self?.copy(from: .left) })
        m.addItem(menuItem("Copy to Left  ⌥←", enabled: n.right != nil) { [weak self] in self?.copy(from: .right) })
        m.addItem(menuItem("Move to Right  ⌃⌥R", enabled: n.left != nil) { [weak self] in self?.transfer(from: .left, move: true) })
        m.addItem(menuItem("Move to Left  ⌃⌥L", enabled: n.right != nil) { [weak self] in self?.transfer(from: .right, move: true) })
        m.addItem(.separator())
        m.addItem(menuItem("Rename…  F2", enabled: onSide) { [weak self] in self?.beginRename() })
        m.addItem(menuItem("New Folder…  ⌘⇧N") { [weak self] in self?.newFolder() })
        m.addItem(menuItem("Move to Trash  ⌘⌫", enabled: onSide) { [weak self] in self?.trash() })
        m.addItem(.separator())
        m.addItem(menuItem("Copy Path", enabled: onSide) { [weak self] in self?.copyPaths() })
        m.addItem(menuItem("Reveal in Finder", enabled: onSide) {
            if let p = path { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)]) }
        })
        if !isFolder {
            m.addItem(menuItem("Open in Notes", enabled: onSide) { [weak self] in if let p = path { self?.host?.folderOpenNote(p) } })
        }
        m.addItem(menuItem("Open in Default App", enabled: onSide) { if let p = path { NSWorkspace.shared.open(URL(fileURLWithPath: p)) } })
        m.addItem(menuItem("Quick Look  Space", enabled: n.left != nil || n.right != nil) { [weak self] in self?.toggleQuickLook() })
        if isFolder {
            m.addItem(.separator())
            m.addItem(menuItem("Set as Base Folder  ⌘↓", enabled: n.left != nil && n.right != nil) { [weak self] in self?.setBase(row, side: nil) })
            m.addItem(menuItem("Set as Left Base Folder", enabled: n.left?.isDir == true) { [weak self] in self?.setBase(row, side: .left) })
            m.addItem(menuItem("Set as Right Base Folder", enabled: n.right?.isDir == true) { [weak self] in self?.setBase(row, side: .right) })
        }
        m.addItem(.separator())
        m.addItem(menuItem("Exclude “\(n.name)”") { [weak self] in
            s.extraExclude.append(n.name)
            self?.rescan()
        })
        return m
    }

    // MARK: drag out / across, drops

    // a drag from a row: the selection's files on that side (a row outside
    // the selection becomes the selection)
    private func dragOut(_ row: Int, _ side: CompareSide, _ e: NSEvent) {
        guard let s = session, let t = s.tree, let n = node(at: row) else { return }
        if !s.marked.contains(n.id) { s.marked = []; s.cursor = row; s.anchor = nil }
        s.focus = side
        sync()
        let paths = targets().filter { (side == .left ? $0.left : $0.right) != nil }.map { t.path($0, side) }
        guard !paths.isEmpty else { return }
        tree.beginDrag(paths, side: side, row: row, event: e)
    }

    // our own rows dropped on the other side = Copy (Cmd: Move) to that side,
    // mirrored paths; files from elsewhere land in the folder under the drop
    // (a folder row on that side, else the row's folder, else the root)
    func drop(_ side: CompareSide, _ row: Int?, _ urls: [URL], own: Bool, move: Bool) -> Bool {
        guard let s = session, let t = s.tree else { return false }
        if own {
            transfer(from: side.other, move: move)
            return true
        }
        guard !urls.isEmpty else { return false }
        var dir = side == .left ? s.leftRoot : s.rightRoot
        if let row, let n = node(at: row), let info = side == .left ? n.left : n.right {
            let p = t.path(n, side)
            dir = info.isDir ? p : (p as NSString).deletingLastPathComponent
        }
        let word = "\(move ? "moved" : "copied") \(urls.count) item\(urls.count == 1 ? "" : "s") into \(tilde(dir))"
        s.status = "working…"
        sync()
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let out = FileOps.transfer(urls, into: dir, move: move, undo: s.undo)
            DispatchQueue.main.async { self?.finish(s, word, out) }
        }
        return true
    }

    // MARK: Synchronize (Update / Mirror; always through a preview sheet)

    func showSyncMenu() {
        let m = NSMenu()
        for mode in SyncMode.allCases {
            if mode == .mirrorRight { m.addItem(.separator()) }
            m.addItem(menuItem(mode.title + "…") { [weak self] in self?.previewSync(mode) })
        }
        let b = syncButton!
        m.popUp(positioning: nil, at: NSPoint(x: b.frame.minX, y: b.frame.maxY + 4), in: toolbar)
    }

    func previewSync(_ mode: SyncMode) {
        guard let s = session, let t = s.tree, let host else { return }
        if s.scanning || s.checking { s.status = "wait for the scan to finish, then synchronize"; sync(); return }
        let plan = SyncPlan.make(t, mode, nameFilter: s.view.nameFilter)
        lastPlan = (mode, plan)
        guard !plan.isEmpty else {
            s.status = "\(mode.title): nothing to do" + (plan.skipped.isEmpty ? "" : " (\(plan.skipped.count) differ with neither side newer)")
            sync()
            return
        }
        let toR = plan.copies.filter { $0.to == .right }.count, toL = plan.copies.count - toR
        let replaces = plan.copies.filter(\.replaces).count
        // the Trash leads: it is the part of a run you can lose files to
        var head: [String] = []
        if !plan.trash.isEmpty { head.append("\(plan.trash.count) to the Trash") }
        if toR > 0 { head.append("\(toR) to the right") }
        if toL > 0 { head.append("\(toL) to the left") }
        let cfg = host.folderConfig
        let risky = !plan.trash.isEmpty || replaces > 0
        // Cancel is the default (Return); the run's button is red when it
        // trashes or overwrites anything
        host.folderConfirm(
            "\(mode.title): " + head.joined(separator: ", "),
            info: mode.explain + " " + cfg.label("sync-info", "Replaced and removed items go to the Trash; ⌘Z undoes the whole run."),
            accessory: syncPreview(plan, replaces: replaces),
            choices: [(cfg.label("sync-run-button", "Synchronize"), risky ? .danger : .normal),
                      (cfg.label("cancel-button", "Cancel"), .primary)],
            defaultIndex: 1, cancelIndex: 1) { [weak self] i in
            if i == 0 { self?.runSync(mode, plan) }
        }
    }

    // the run in a recessed well, Trash first: what goes, what is
    // overwritten, then the rest (palette hues lifted to text contrast)
    private func syncPreview(_ plan: SyncPlan, replaces: Int) -> NSView {
        let c = colors
        let font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        let bold = NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold)
        let wellBg = c.over(c.mantle, c.isLight ? 0.55 : 0.6, on: ConfirmOverlay.fill(c))    // = ButtonStyle.inputFill
        let danger = c.ensure(c.tone(.danger), on: wellBg), warn = c.ensure(c.tone(.warning), on: wellBg)
        let plain = c.ensure(c.text, on: wellBg), quiet = c.ensure(c.over(c.text, 0.7, on: wellBg), on: wellBg)
        let out = NSMutableAttributedString()
        func line(_ t: String, _ col: NSColor, _ f: NSFont? = nil) {
            if out.length > 0 { out.append(NSAttributedString(string: "\n")) }
            out.append(NSAttributedString(string: t, attributes: [.font: f ?? font, .foregroundColor: col]))
        }
        let cap = 14
        var shown = 0
        if !plan.trash.isEmpty {
            line("Moved to the Trash (\(plan.trash.count))", danger, bold)
            for t in plan.trash.prefix(cap) { line("  ✕  \(t.side.rawValue): \(t.rel)", danger); shown += 1 }
        }
        let ordered = plan.copies.filter(\.replaces) + plan.copies.filter { !$0.replaces }
        if !ordered.isEmpty, shown < cap {
            line(replaces > 0 ? "Copied (\(plan.copies.count), \(replaces) overwrite — old copies to the Trash)"
                              : "Copied (\(plan.copies.count))", plain, bold)
            for cp in ordered.prefix(cap - shown) {
                line("  \(cp.to == .right ? "→" : "←")  \(cp.rel)" + (cp.replaces ? "  (overwrites)" : ""),
                     cp.replaces ? warn : plain)
                shown += 1
            }
        }
        let total = plan.copies.count + plan.trash.count
        if total > shown { line("  … and \(total - shown) more", quiet) }
        if !plan.skipped.isEmpty { line("Skipped (differ, neither newer): \(plan.skipped.count)", quiet) }
        let well = NSView()
        well.wantsLayer = true
        well.layer?.backgroundColor = ButtonStyle.inputFill(c).cgColor
        well.layer?.borderColor = ButtonStyle.inputStroke(c).cgColor
        well.layer?.borderWidth = 1
        well.layer?.cornerRadius = 6
        let f = NSTextField(labelWithAttributedString: out)
        f.lineBreakMode = .byTruncatingMiddle
        f.isSelectable = false
        let h = ceil(f.intrinsicContentSize.height)
        f.frame = NSRect(x: 10, y: 8, width: 424, height: h)
        f.autoresizingMask = [.width]
        well.frame.size = NSSize(width: 444, height: h + 16)
        well.addSubview(f)
        return well
    }

    // copies (old targets → Trash), then the Trash; ONE undo step
    private func runSync(_ mode: SyncMode, _ plan: SyncPlan) {
        guard let s = session else { return }
        s.status = "synchronizing…"
        sync()
        let stack = s.undo
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let mark = stack.count
            var out = FileOps.Outcome()
            for side in [CompareSide.right, .left] {
                let items = plan.copies.filter { $0.to == side }.map { ($0.src, $0.dst) }
                guard !items.isEmpty else { continue }
                let o = FileOps.place(items, move: false, clash: .replace, undo: stack)
                out.changes += o.changes
                if out.failed == nil { out.failed = o.failed }
            }
            if !plan.trash.isEmpty {
                let o = FileOps.trash(plan.trash.map(\.path), undo: stack)
                out.changes += o.changes
                if out.failed == nil { out.failed = o.failed }
            }
            stack.collapse(since: mark, mode.title.lowercased())
            let word = "\(mode.title): \(plan.copies.count) copied" + (plan.trash.isEmpty ? "" : ", \(plan.trash.count) trashed") + " (⌘Z undoes)"
            DispatchQueue.main.async { self?.finish(s, word, out) }
        }
    }

    // MARK: Quick Look (Space)

    private var quickLookUp: Bool {
        QLPreviewPanel.sharedPreviewPanelExists() && QLPreviewPanel.shared().isVisible
            && QLPreviewPanel.shared().dataSource === self
    }

    // the selection's files on the focused side (the other side's when the
    // focused one has none)
    private var previewPaths: [String] {
        guard let s = session, let t = s.tree else { return [] }
        return targets().compactMap { n in
            if (s.focus == .left ? n.left : n.right) != nil { return t.path(n, s.focus) }
            if (s.focus == .left ? n.right : n.left) != nil { return t.path(n, s.focus.other) }
            return nil
        }
    }

    func toggleQuickLook() {
        guard let panel = QLPreviewPanel.shared() else { return }
        if quickLookUp {
            panel.orderOut(nil)
            host?.folderWindow.makeKey()
            return
        }
        quickLookPaths = previewPaths
        guard !quickLookPaths.isEmpty else { return }
        panel.dataSource = self
        panel.delegate = self
        panel.reloadData()
        panel.currentPreviewItemIndex = 0
        panel.makeKeyAndOrderFront(nil)
    }

    private func refreshQuickLook() {
        guard quickLookUp else { return }
        quickLookPaths = previewPaths
        QLPreviewPanel.shared().reloadData()
    }

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { quickLookPaths.count }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! {
        guard quickLookPaths.indices.contains(index) else { return nil }
        return URL(fileURLWithPath: quickLookPaths[index]) as NSURL
    }
    // the panel holds the keyboard: Space / Esc close it, ↑↓ step the rows
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown, let s = session else { return false }
        switch event.keyCode {
        case 49, 53: toggleQuickLook(); return true
        case 125: move(to: s.cursor + 1); refreshQuickLook(); return true
        case 126: move(to: s.cursor - 1); refreshQuickLook(); return true
        default: return false
        }
    }

    // MARK: test hooks

    var testState: [String: Any] {
        guard let s = session else { return [:] }
        let c = s.tree?.counts() ?? FolderTree.Counts()
        return [
            "left": s.leftRoot, "right": s.rightRoot, "scanning": s.scanning, "checking": s.checking,
            "filter": s.view.filter.rawValue, "flatten": s.view.flatten, "nameFilter": s.view.nameFilter,
            "cursor": s.cursor, "focus": s.focus.rawValue, "marked": s.marked.count,
            "summary": summary.stringValue, "status": s.status, "items": s.tree?.all.count ?? 0,
            "counts": ["different": c.different, "unimportant": c.unimportant, "leftOnly": c.leftOnly,
                       "rightOnly": c.rightOnly, "same": c.same, "unknown": c.unknown],
            "scanMs": Int(s.scanMs), "canUndo": s.undo.canUndo, "sharedCanUndo": FileOps.canUndo,
            "back": s.back.count, "forward": s.forward.count, "quickLook": quickLookUp ? quickLookPaths : [],
            "dropSide": tree.dropSide?.rawValue ?? NSNull(),
            "syncPlan": lastPlan.map { p -> Any in
                ["mode": p.mode.rawValue, "copies": p.plan.copies.map { "\($0.to == .right ? "→" : "←") \($0.rel)" },
                 "trash": p.plan.trash.map { "\($0.side.rawValue): \($0.rel)" }, "skipped": p.plan.skipped] as [String: Any]
            } ?? NSNull(),
            "rows": s.rows.prefix(200).map { r -> [String: Any] in
                ["rel": r.node.rel, "depth": r.depth, "status": r.node.status.rawValue, "newer": r.node.newer.rawValue,
                 "dir": r.node.isDir, "expanded": r.node.expanded,
                 "left": r.node.left != nil, "right": r.node.right != nil]
            },
        ]
    }

    // do:compare:folder-…
    func testDo(_ a: String, arg: String) -> String? {
        guard let s = session else { return "no folder session" }
        switch a {
        case "filter":
            guard let f = FolderFilter(rawValue: arg) else { return "folder-filter:\(FolderFilter.allCases.map(\.rawValue).joined(separator: "|"))" }
            setFilter(f)
        case "flatten": toggleFlatten()
        case "names": nameBox.field.stringValue = arg; setNameFilter(arg)
        case "expand": t_expandAll(arg != "none")
        case "select":
            guard let i = s.rows.firstIndex(where: { $0.node.rel == arg }) else { return "no row \(arg)" }
            move(to: i)
        case "copy", "move":
            // folder-copy:right[:replace|keep|skip] = copy TO that side
            let bits = arg.split(separator: ":").map(String.init)
            testClash = bits.count > 1 ? ["replace": .replace, "keep": .keepBoth, "skip": .skip][bits[1]] : nil
            defer { testClash = nil }
            transfer(from: bits.first == "left" ? .right : .left, move: a == "move")
        case "trash": trash()
        case "undo": undo()
        case "open": activate(s.cursor)
        case "focus": s.focus = arg == "right" ? .right : .left; sync()
        case "rescan": rescan()
        case "hidden": s.hidden.toggle(); rescan()
        case "base":
            // folder-base:REL[:left|right] = Set as (Left / Right) Base Folder on that row
            let bits = arg.split(separator: ":").map(String.init)
            guard let rel = bits.first, let i = s.rows.firstIndex(where: { $0.node.rel == rel }) else { return "no row \(arg)" }
            setBase(i, side: bits.count > 1 ? CompareSide(rawValue: bits[1]) : nil)
        case "up": upOneLevel()
        case "back": goBack()
        case "forward": goForward()
        case "quicklook": toggleQuickLook()
        case "sync":
            // folder-sync:MODE[:preview] — preview fills state.syncPlan only; else it runs (no sheet)
            let bits = arg.split(separator: ":").map(String.init)
            guard let m = bits.first.flatMap(SyncMode.init(rawValue:)) else {
                return "folder-sync:" + SyncMode.allCases.map(\.rawValue).joined(separator: "|")
            }
            guard let t = s.tree else { return "no tree yet" }
            let plan = SyncPlan.make(t, m, nameFilter: s.view.nameFilter)
            lastPlan = (m, plan)
            if bits.count < 2 { runSync(m, plan) }
        case "drop":
            // folder-drop:SIDE:PATH[,PATH] = files dropped on that side's root (as from Finder);
            // folder-drop:SIDE = the selection dragged across onto that side
            let bits = arg.split(separator: ":", maxSplits: 1).map(String.init)
            guard let side = bits.first.flatMap(CompareSide.init(rawValue:)) else { return "folder-drop:left|right[:PATHS]" }
            if bits.count > 1 {
                _ = drop(side, nil, bits[1].split(separator: ",").map { URL(fileURLWithPath: String($0)) }, own: false, move: false)
            } else {
                _ = drop(side, nil, [], own: true, move: false)
            }
        default: return "unknown folder action \(a)"
        }
        return nil
    }
}

// MARK: - Ctrl+H/J/K/L panes (PaneNav.swift): the tree's two sides, the
// name filter when shown
extension FolderPage {
    var navPanes: [NavPane] {
        var out: [NavPane] = []
        if !nameBox.isHidden { out.append(.area("names", nameBox)) }
        for side in [CompareSide.left, .right] {
            out.append(NavPane(side.rawValue, scroll, part: { [weak self] in
                guard let self else { return .zero }
                let v = self.scroll.documentVisibleRect
                let r = NSRect(x: self.tree.sideX(side), y: v.minY, width: self.tree.paneW, height: v.height)
                return self.scroll.convert(r, from: self.tree)
            }, focus: { [weak self] in
                guard let self, let s = self.session else { return }
                s.focus = side
                self.focusTree()
                self.sync()
            }, owns: { [weak self] r in
                guard let self else { return false }
                return NavPane.inside(r, self.scroll) && self.session?.focus == side
            }))
        }
        return out
    }
}
