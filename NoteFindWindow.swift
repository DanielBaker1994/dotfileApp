import AppKit

// MARK: - Notes pad: leader + s f ("search files")
//
// In the notes vim pane, Space s f (vim/init.lua) sends `notes-find` to the
// daemon's socket; this popup lists every file under `[notes-find] roots`
// (default: the folders / files of `[notes] paths`), newest first, and
// filters them as you type (fuzzy: the letters in order, basename matches
// first). Return opens the file as a notes tab (`openNoteFile`), Esc closes.
// A tool panel like /paths: borderless, non-activating, never part of the
// shared window.
enum NoteFinder {
    struct Hit { var path: String; var rel: String; var mtime: Double; var tab: Int }

    // fuzzy: every query letter in order. Lower = better; nil = no match.
    // A match inside the basename beats one in the folders; tight spans and
    // early starts win; the caller breaks ties by recency.
    static func score(_ hit: Hit, _ q: String) -> Int? {
        if q.isEmpty { return 0 }
        let rel = Array(hit.rel.lowercased())
        let nameStart = (hit.rel.lowercased() as NSString).deletingLastPathComponent.count
            + ((hit.rel as NSString).deletingLastPathComponent.isEmpty ? 0 : 1)
        var i = 0, first = -1, last = -1
        for ch in q {
            guard let j = rel[i...].firstIndex(of: ch) else { return nil }
            if first < 0 { first = j }
            last = j; i = j + 1
        }
        let span = last - first
        let inName = first >= nameStart
        return (inName ? 0 : 1000) + span * 4 + max(0, first - (inName ? nameStart : 0))
    }
}

final class NoteFindWindow: NSObject {
    let window: PopupWindow
    private let list: FileListPane
    private let scroll = NSScrollView()
    private let empty = NSTextField(labelWithString: "")
    private let colors: PopupColors
    private var all: [NoteFinder.Hit] = []
    private var shown: [NoteFinder.Hit] = []
    private var query = ""
    private var iconCache: [String: NSImage] = [:]
    private static let rowH: CGFloat = 22

    var onOpen: ((String) -> Void)?
    var openPaths: (() -> [String])?       // the notes' open tabs
    var log: ((String) -> Void)?

    init(_ cmd: CommandSpec?) {
        colors = cmd.map { windowColors($0) } ?? windowColors()
        var cfg = PopupConfig(name: "notes-find")
        let w = Double(configSectionValue("notes-find", "width") ?? "") ?? 640
        cfg.enableToggle = false
        cfg.enableDrag = false
        cfg.dynamicHeight = false
        cfg.enableNavigation = false
        cfg.sticky = false
        cfg.toolPanel = true
        cfg.floating = true
        cfg.width = CGFloat(max(360, w))
        cfg.colors = colors
        cfg.showSearchBar = true
        cfg.searchWidthFraction = 1
        cfg.searchPlaceholder = configSectionValue("notes-find", "placeholder")
            ?? "open notes — type to filter · ↩ switch · esc close"
        window = PopupWindow(config: cfg)
        list = FileListPane(config: cfg)
        super.init()
        window.onFilter = { [weak self] q in
            self?.query = q
            if self?.window.isShown == true { self?.reload() }
            return []
        }
        window.onEscape = { [weak self] in self?.hide() }
        window.onCloseWindow = { [weak self] in self?.hide() }
        window.onKeyPreview = { [weak self] code, mods in self?.key(code, mods) ?? false }
        list.dropDirectory = { nil }
        list.canPerform = { a in [.copy].contains(a) }
        list.onOpen = { [weak self] i in self?.open(i) }
        list.onCopyPath = { [weak self] i in self?.copyPath(i) }
        list.onOpenInNotes = { [weak self] i in self?.open(i) }
        list.onFocusSearch = { [weak self] chars in
            guard let self else { return }
            self.window.focusSearchField()
            self.window.nativeWindow.fieldEditor(false, for: nil)?.insertText(chars)
        }
        scroll.documentView = list
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        empty.font = .systemFont(ofSize: 12)
        empty.textColor = colors.dim
        empty.isHidden = true
    }

    var isShown: Bool { window.isShown }

    func show() {
        if window.isShown {
            window.nativeWindow.orderFrontRegardless()
            window.nativeWindow.makeKey()
            window.focusSearchField()
            return
        }
        if window.nativeWindow.contentView?.subviews.contains(scroll) == true {
            window.showPersistent()
        } else {
            window.show()
            if let backdrop = window.nativeWindow.contentView {
                backdrop.addSubview(scroll)
                backdrop.addSubview(empty)
            }
        }
        query = window.currentSearchText.trimmingCharacters(in: .whitespaces)
        reload()
        place()
        window.focusSearchField()
        rescan()
    }

    func hide() {
        guard window.isShown else { return }
        window.hide(restore: true)
    }

    // the notes' OPEN tabs only (tab order = the tie-break order)
    private func rescan() {
        let fm = FileManager.default
        let paths = openPaths?() ?? []
        all = paths.enumerated().map { i, p in
            let m = (try? fm.attributesOfItem(atPath: p)[.modificationDate] as? Date)??.timeIntervalSince1970 ?? 0
            return NoteFinder.Hit(path: p, rel: Self.shortPath(p), mtime: m, tab: i)
        }
        reload()
        log?("notes-find: \(all.count) open notes")
    }

    private static func shortPath(_ p: String) -> String {
        let home = NSHomeDirectory()
        return p.hasPrefix(home + "/") ? "~" + p.dropFirst(home.count) : p
    }

    private func reload() {
        let q = query.lowercased().filter { !$0.isWhitespace }
        let scored: [(NoteFinder.Hit, Int)] = all.compactMap { h in
            NoteFinder.score(h, q).map { (h, $0) }
        }
        let limit = max(5, Int(configSectionValue("notes-find", "rows") ?? "") ?? 12)
        shown = Array(scored.sorted { $0.1 != $1.1 ? $0.1 < $1.1 : $0.0.tab < $1.0.tab }
            .prefix(limit).map { $0.0 })
        let now = Date().timeIntervalSince1970
        let font = NSFont.systemFont(ofSize: 10)
        list.rows = shown.map { h in
            let dir = (h.rel as NSString).deletingLastPathComponent
            let trailing = (dir.isEmpty ? "" : dir + " · ") + PathsWindow.age(now - h.mtime)
            return PopupFileBrowser.Entry(name: (h.path as NSString).lastPathComponent, path: h.path,
                                          isDir: false, size: 0, icon: icon(h.path), trailingText: trailing,
                                          trailingWidth: ceil((trailing as NSString).size(withAttributes: [.font: font]).width))
        }
        list.selection = 0
        empty.stringValue = all.isEmpty ? "No notes are open" : "No file matches “\(query)”"
        layoutList()
    }

    private func icon(_ path: String) -> NSImage {
        let ext = (path as NSString).pathExtension.lowercased()
        if let i = iconCache[ext] { return i }
        let i = NSWorkspace.shared.icon(forFile: path)
        i.size = NSSize(width: 16, height: 16)
        iconCache[ext] = i
        return i
    }

    private func layoutList() {
        guard let backdrop = window.nativeWindow.contentView else { return }
        let top = window.searchFieldFrame.maxY + 8
        let n = CGFloat(max(1, shown.count))
        let vis = (window.nativeWindow.screen ?? NSScreen.main)?.visibleFrame.height ?? 900
        let listH = min(n * Self.rowH, vis * 0.6)
        let total = ceil(top + listH + 10)
        var f = window.nativeWindow.frame
        if abs(f.height - total) > 0.5 {
            f.origin.y += f.height - total
            f.size.height = total
            window.nativeWindow.setFrame(f, display: true)
        }
        scroll.frame = NSRect(x: 4, y: top, width: backdrop.bounds.width - 8, height: listH)
        list.frame = NSRect(x: 0, y: 0, width: scroll.contentSize.width,
                            height: max(listH, CGFloat(shown.count) * Self.rowH))
        list.needsDisplay = true
        empty.isHidden = !shown.isEmpty
        empty.frame = NSRect(x: 16, y: top + 3, width: backdrop.bounds.width - 32, height: 16)
        scrollToSelection()
    }

    // the screen with the mouse, top a fifth down (launcher position)
    private func place() {
        let mouse = NSEvent.mouseLocation
        guard let vis = (NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main)?.visibleFrame else { return }
        var f = window.nativeWindow.frame
        f.origin.x = vis.midX - f.width / 2
        f.origin.y = max(vis.minY, vis.maxY - vis.height * 0.2 - f.height)
        window.nativeWindow.setFrame(f, display: true)
    }

    private func scrollToSelection() {
        guard list.rows.indices.contains(list.selection) else { return }
        list.scrollToVisible(list.rowRect(list.selection))
    }

    private func key(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        let editor = window.nativeWindow.firstResponder as? NSTextView
        let textSelected = (editor?.selectedRange().length ?? 0) > 0
        switch code {
        case 53: hide(); return true                                // Esc
        case 13 where cmd: hide(); return true                      // Cmd+W
        case 125, 45 where ctrl: move(1); return true               // ↓ / Ctrl+N
        case 126, 35 where ctrl: move(-1); return true              // ↑ / Ctrl+P
        case 36, 76: open(list.selection); return true              // Return
        case 8 where cmd && !textSelected:                          // Cmd+C: the path
            copyPath(list.selection); return true
        default: return false
        }
    }

    private func move(_ d: Int) {
        guard !shown.isEmpty else { return }
        list.moveSelection(d)
        scrollToSelection()
    }

    private func open(_ i: Int) {
        guard shown.indices.contains(i) else { NSSound.beep(); return }
        let p = shown[i].path
        hide()
        onOpen?(p)
        log?("notes-find: open \(p)")
    }

    private func copyPath(_ i: Int) {
        guard shown.indices.contains(i) else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(shown[i].path, forType: .string)
        window.showToast("Copied \(PathsWindow.folder(shown[i].path))/\((shown[i].path as NSString).lastPathComponent)",
                         symbol: "checkmark.circle.fill", centered: true)
    }
}
