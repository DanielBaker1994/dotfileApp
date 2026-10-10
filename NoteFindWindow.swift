import AppKit

enum NoteFinder {
    struct Hit { var path: String; var rel: String; var mtime: Double; var tab: Int }

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
    enum Mode { case files, grep }
    let mode: Mode
    private var grepGen = 0
    private var lines: [Int] = []
    private var grepNote = ""
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

    var onOpen: ((String, Int?) -> Void)?
    var openPaths: (() -> [String])?
    var log: ((String) -> Void)?

    init(_ cmd: CommandSpec?, mode: Mode = .files) {
        self.mode = mode
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
        cfg.searchPlaceholder = mode == .files
            ? "open notes — type to filter · ↩ switch · esc close"
            : "search the open notes (ripgrep) — ↩ jump to the line · esc close"
        window = PopupWindow(config: cfg)
        list = FileListPane(config: cfg)
        super.init()
        window.onFilter = { [weak self] q in
            self?.query = q
            if self?.window.isShown == true { self?.queryChanged() }
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
        place()
        window.focusSearchField()
        if mode == .files { rescan() } else { queryChanged() }
    }

    func hide() {
        guard window.isShown else { return }
        window.hide(restore: true)
    }

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

    private func queryChanged() {
        if mode == .files { reload() } else { runGrep() }
    }

    private func runGrep() {
        grepGen += 1
        let gen = grepGen
        let q = query.trimmingCharacters(in: .whitespaces)
        let paths = (openPaths?() ?? []).filter { FileManager.default.fileExists(atPath: $0) }
        guard q.count >= 2, !paths.isEmpty else {
            grepNote = paths.isEmpty ? "No notes are open" : "Type 2+ characters to search \(paths.count) open notes"
            showGrep([])
            return
        }
        let rg = Self.rgPath()
        let cap = max(1, Int(configSectionValue("notes-find", "grep-max") ?? "") ?? 200)
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.12) { [weak self] in
            guard let self, gen == self.grepGen else { return }
            guard let rg else {
                DispatchQueue.main.async { self.grepNote = "ripgrep (rg) not found — brew install ripgrep"; self.showGrep([]) }
                return
            }
            let sep = "\u{1F}"
            let t0 = DispatchTime.now().uptimeNanoseconds
            let r = try? runProcess(rg, ["--line-number", "--no-heading", "--with-filename", "--color", "never",
                                         "--smart-case", "--max-columns", "300", "--max-columns-preview",
                                         "--field-match-separator", sep, "-e", q, "--"] + paths)
            var hits: [(String, Int, String)] = []
            for l in (r?.out ?? "").split(separator: "\n", omittingEmptySubsequences: true) {
                let p = l.split(separator: Character(sep), maxSplits: 2, omittingEmptySubsequences: false)
                guard p.count == 3, let n = Int(p[1]) else { continue }
                hits.append((String(p[0]), n, p[2].trimmingCharacters(in: .whitespaces)))
                if hits.count >= cap { break }
            }
            let order = Dictionary(uniqueKeysWithValues: paths.enumerated().map { ($1, $0) })
            hits.sort { (order[$0.0] ?? 0, $0.1) < (order[$1.0] ?? 0, $1.1) }
            DispatchQueue.main.async {
                guard gen == self.grepGen else { return }
                self.grepNote = "No match for “\(q)”"
                self.showGrep(hits)
                let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
                self.log?(String(format: "notes-grep: '%@' %d hits, %.0f ms", q, hits.count, ms))
            }
        }
    }

    static func rgPath() -> String? {
        let c = configSectionValue("notes-find", "rg-bin").flatMap { $0.isEmpty ? nil : ($0 as NSString).expandingTildeInPath }
        return ([c].compactMap { $0 } + ["/opt/homebrew/bin/rg", "/usr/local/bin/rg", "/usr/bin/rg"])
            .first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    private func showGrep(_ hits: [(String, Int, String)]) {
        let font = NSFont.systemFont(ofSize: 10)
        shown = hits.map { NoteFinder.Hit(path: $0.0, rel: ($0.0 as NSString).lastPathComponent, mtime: 0, tab: 0) }
        lines = hits.map { $0.1 }
        let rows = max(5, Int(configSectionValue("notes-find", "rows") ?? "") ?? 12)
        list.rows = hits.map { h in
            let trailing = "\((h.0 as NSString).lastPathComponent):\(h.1)"
            return PopupFileBrowser.Entry(name: h.2.isEmpty ? "(blank)" : h.2, path: h.0, isDir: false, size: 0,
                                          icon: icon(h.0), trailingText: trailing,
                                          trailingWidth: ceil((trailing as NSString).size(withAttributes: [.font: font]).width))
        }
        list.selection = 0
        empty.stringValue = grepNote
        layoutList(maxRows: rows)
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

    private func layoutList(maxRows: Int? = nil) {
        guard let backdrop = window.nativeWindow.contentView else { return }
        let top = window.searchFieldFrame.maxY + 8
        let n = CGFloat(max(1, min(shown.count, maxRows ?? Int.max)))
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
        case 53: hide(); return true
        case 13 where cmd: hide(); return true
        case 125, 45 where ctrl: move(1); return true
        case 126, 35 where ctrl: move(-1); return true
        case 36, 76: open(list.selection); return true
        case 8 where cmd && !textSelected:
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
        let line = mode == .grep && lines.indices.contains(i) ? lines[i] : nil
        hide()
        onOpen?(p, line)
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
