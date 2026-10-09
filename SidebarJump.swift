import AppKit

// MARK: - Jira sidebar jump: Space s f
//
// The notes pad's Space s f for the Jira sidebar: a popup listing every row
// of the left panel (My work, Pinned, Boards, Synced),
// filtered as you type (the letters in order; title beats section). Return
// does what clicking that row does. Esc closes. A tool panel
// like NoteFindWindow: borderless, non-activating, never part of the shared
// window. `[notes-find] rows / width` size it too.
final class SidebarJumpWindow: NSObject {
    let window: PopupWindow
    private let list: FileListPane
    private let scroll = NSScrollView()
    private let empty = NSTextField(labelWithString: "")
    private let colors: PopupColors
    private var all: [PopupWindow.SidebarJumpItem] = []
    private var shown: [PopupWindow.SidebarJumpItem] = []
    private var query = ""
    private var iconCache: [String: NSImage] = [:]
    private static let rowH: CGFloat = 22

    var onJump: ((Int) -> Void)?

    init(_ cmd: CommandSpec?) {
        colors = cmd.map { windowColors($0) } ?? windowColors()
        var cfg = PopupConfig(name: "sidebar-jump")
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
        cfg.searchPlaceholder = "jump to a sidebar item — type to filter · ↩ open · esc close"
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
        list.canPerform = { _ in false }
        list.onOpen = { [weak self] i in self?.open(i) }
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

    func show(items: [PopupWindow.SidebarJumpItem]) {
        all = items
        if window.isShown {
            window.nativeWindow.orderFrontRegardless()
            window.nativeWindow.makeKey()
            window.focusSearchField()
            reload()
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
        reload()
    }

    func hide() {
        guard window.isShown else { return }
        window.hide(restore: true)
    }

    // the query's letters in order; a match in the title beats one in the
    // section. Lower = better; nil = no match.
    private static func score(_ item: PopupWindow.SidebarJumpItem, _ q: String) -> Int? {
        if q.isEmpty { return 0 }
        func span(_ s: String) -> Int? {
            let chars = Array(s.lowercased())
            var i = 0, first = -1, last = -1
            for ch in q {
                guard let j = chars[i...].firstIndex(of: ch) else { return nil }
                if first < 0 { first = j }
                last = j; i = j + 1
            }
            return (last - first) * 4 + first
        }
        if let s = span(item.title) { return s }
        return span(item.section + " " + item.title).map { 1000 + $0 }
    }

    private func reload() {
        let q = query.lowercased().filter { !$0.isWhitespace }
        let scored = all.enumerated().compactMap { i, it in Self.score(it, q).map { (it, $0, i) } }
        shown = scored.sorted { $0.1 != $1.1 ? $0.1 < $1.1 : $0.2 < $1.2 }.map { $0.0 }
        let font = NSFont.systemFont(ofSize: 10)
        list.rows = shown.map { it in
            PopupFileBrowser.Entry(name: it.title, path: String(it.row), isDir: false, size: 0,
                                   icon: icon(it), trailingText: it.section,
                                   trailingWidth: ceil((it.section as NSString).size(withAttributes: [.font: font]).width))
        }
        list.selection = 0
        empty.stringValue = all.isEmpty ? "The sidebar is empty" : "No sidebar item matches “\(query)”"
        layoutList()
    }

    // a tinted chip per section (the header's workspace chips, same idea):
    // the glyph in the section's color on a faint square of it
    private func tone(_ section: String) -> PopupTone {
        switch section {
        case "My work": return .accent
        case "Boards": return .info
        case "Pinned": return .warning
        default: return .accent2
        }
    }

    private func icon(_ item: PopupWindow.SidebarJumpItem) -> NSImage {
        let name = item.icon ?? "list.bullet.rectangle"
        let key = name + "|" + item.section
        if let i = iconCache[key] { return i }
        let color = colors.tone(tone(item.section))
        let cfg = NSImage.SymbolConfiguration(pointSize: 11, weight: .medium)
            .applying(NSImage.SymbolConfiguration(paletteColors: [color]))
        let glyph = (NSImage(systemSymbolName: name, accessibilityDescription: nil)
            ?? NSImage(systemSymbolName: "list.bullet.rectangle", accessibilityDescription: nil))?
            .withSymbolConfiguration(cfg)
        let chip = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { r in
            color.withAlphaComponent(0.18).setFill()
            NSBezierPath(roundedRect: r, xRadius: 5, yRadius: 5).fill()
            if let g = glyph {
                let s = g.size
                g.draw(in: NSRect(x: r.midX - s.width / 2, y: r.midY - s.height / 2, width: s.width, height: s.height))
            }
            return true
        }
        iconCache[key] = chip
        return chip
    }

    private func layoutList() {
        guard let backdrop = window.nativeWindow.contentView else { return }
        let maxRows = max(5, Int(configSectionValue("notes-find", "rows") ?? "") ?? 12)
        let top = window.searchFieldFrame.maxY + 8
        let n = CGFloat(max(1, min(shown.count, maxRows)))
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
        switch code {
        case 53: hide(); return true                                // Esc
        case 13 where cmd: hide(); return true                      // Cmd+W
        case 125, 45 where ctrl: move(1); return true               // ↓ / Ctrl+N
        case 126, 35 where ctrl: move(-1); return true              // ↑ / Ctrl+P
        case 36, 76: open(list.selection); return true              // Return
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
        let row = shown[i].row
        hide()
        onJump?(row)
    }
}
