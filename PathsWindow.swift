import AppKit
import Quartz

final class PathsWindow: NSObject, QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    let window: PopupWindow
    private let list: FileListPane
    private let scroll = NSScrollView()
    private let empty = NSTextField(labelWithString: "")
    private let colors: PopupColors
    private var shown: [PathShelf.Item] = []
    private var query = ""
    private var returnAction: String
    private var iconCache: [String: NSImage] = [:]
    private var quickLookPaths: [String] = []
    private var rename: InlineRename?
    private static let rowH: CGFloat = 22

    var onOpenInNotes: ((String) -> Void)?
    var onOpenTerminal: ((String) -> Void)?
    var onCopied: (() -> Void)?
    var log: ((String) -> Void)?

    init(_ cmd: CommandSpec, returnAction: String) {
        self.returnAction = returnAction
        colors = windowColors(cmd)
        var cfg = PopupConfig(name: "paths")
        cfg.enableToggle = false
        cfg.enableDrag = false
        cfg.dynamicHeight = false
        cfg.enableNavigation = false
        cfg.sticky = cmd.sticky
        cfg.toolPanel = true
        cfg.floating = cmd.float ?? true
        cfg.width = cmd.width > 0 ? cmd.width : 680
        cfg.colors = colors
        cfg.showSearchBar = true
        cfg.searchWidthFraction = 1
        cfg.searchPlaceholder = "recent files — type to filter · ↩ copy file · ⌘C copy path · drag a row out"
        window = PopupWindow(config: cfg)
        list = FileListPane(config: cfg)
        super.init()
        window.onFilter = { [weak self] q in
            self?.setQuery(q)
            return []
        }
        window.onEscape = { [weak self] in self?.hide() }
        window.onCloseWindow = { [weak self] in self?.hide() }
        window.onHide = { _ in
            if QLPreviewPanel.sharedPreviewPanelExists(), QLPreviewPanel.shared().isVisible {
                QLPreviewPanel.shared().orderOut(nil)
            }
        }
        window.onKeyPreview = { [weak self] code, mods in self?.key(code, mods) ?? false }

        list.dropDirectory = { nil }
        list.canPerform = { a in [.quickLook, .copy].contains(a) }
        list.onAction = { [weak self] a in
            switch a {
            case .quickLook: self?.toggleQuickLook()
            case .copy: self?.copyFiles()
            default: break
            }
        }
        list.onOpen = { [weak self] i in self?.open([i]) }
        list.onCopyPath = { [weak self] i in self?.copyPaths([i]) }
        list.onOpenInNotes = { [weak self] i in
            guard let self, self.shown.indices.contains(i) else { return }
            self.onOpenInNotes?(self.shown[i].path)
        }
        list.onOpenTerminal = { [weak self] i in
            guard let self, self.shown.indices.contains(i) else { return }
            self.onOpenTerminal?((self.shown[i].path as NSString).deletingLastPathComponent)
        }
        list.onSelect = { [weak self] _ in self?.refreshQuickLook() }
        list.onRename = { [weak self] i in self?.beginRename(i) }
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

        NotificationCenter.default.addObserver(forName: PathShelf.changed, object: nil, queue: .main) {
            [weak self] _ in
            guard let self, self.window.isShown else { return }
            self.reload(keepSelection: true)
        }
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
        reload(keepSelection: false)
        place()
        window.focusSearchField()
    }

    func hide() {
        guard window.isShown else { return }
        window.hide(restore: true)
    }

    private func setQuery(_ q: String) {
        query = q
        guard window.isShown else { return }
        reload(keepSelection: false)
    }

    func reload(keepSelection: Bool) {
        let selected = list.rows.indices.contains(list.selection) ? list.rows[list.selection].path : nil
        let words = query.lowercased().split(separator: " ").map(String.init)
        shown = PathShelf.shared.entries().filter { item in
            let p = item.path.lowercased()
            return words.allSatisfy { p.contains($0) }
        }
        let now = Date().timeIntervalSince1970
        let font = NSFont.systemFont(ofSize: 10)
        list.rows = shown.map { item in
            let name = (item.path as NSString).lastPathComponent
            let trailing = "\(Self.folder(item.path)) · \(Self.age(now - item.at)) · \(item.why.label)"
            return PopupFileBrowser.Entry(name: name, path: item.path, isDir: false, size: 0,
                                          icon: icon(item.path), trailingText: trailing,
                                          trailingWidth: ceil((trailing as NSString).size(withAttributes: [.font: font]).width))
        }
        if keepSelection, let s = selected, let i = shown.firstIndex(where: { $0.path == s }) {
            list.selection = i
        } else {
            list.selection = 0
        }
        empty.stringValue = query.isEmpty
            ? "Nothing yet — files you save, download or copy show up here"
            : "No recent file matches “\(query)”"
        layoutList()
    }

    private func icon(_ path: String) -> NSImage {
        let ext = (path as NSString).pathExtension.lowercased()
        let key = ["png", "jpg", "jpeg", "heic", "gif", "app"].contains(ext) || ext.isEmpty ? path : ext
        if let i = iconCache[key] { return i }
        let i = NSWorkspace.shared.icon(forFile: path)
        i.size = NSSize(width: 16, height: 16)
        iconCache[key] = i
        return i
    }

    static func folder(_ path: String) -> String {
        var d = (path as NSString).deletingLastPathComponent
        let home = NSHomeDirectory()
        if d == home { return "~" }
        if d.hasPrefix(home + "/") { d = "~" + d.dropFirst(home.count) }
        if d.hasPrefix("/private/tmp") { d = String(d.dropFirst(8)) }
        guard d.count > 34 else { return d }
        let comps = d.split(separator: "/").suffix(2)
        return "…/" + comps.joined(separator: "/")
    }

    static func age(_ s: Double) -> String {
        if s < 60 { return "now" }
        if s < 3600 { return "\(Int(s / 60))m" }
        if s < 86400 { return "\(Int(s / 3600))h" }
        return "\(Int(s / 86400))d"
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

    private func place() {
        let mouse = NSEvent.mouseLocation
        guard let vis = (NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main)?.visibleFrame else { return }
        var f = window.nativeWindow.frame
        f.origin.x = vis.midX - f.width / 2
        f.origin.y = vis.maxY - vis.height * 0.2 - f.height
        f.origin.y = max(vis.minY, f.origin.y)
        window.nativeWindow.setFrame(f, display: true)
    }

    private func scrollToSelection() {
        guard list.rows.indices.contains(list.selection) else { return }
        list.scrollToVisible(list.rowRect(list.selection))
    }

    private func key(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let cmd = mods.contains(.command), ctrl = mods.contains(.control), shift = mods.contains(.shift)
        let editor = window.nativeWindow.firstResponder as? NSTextView
        let textSelected = (editor?.selectedRange().length ?? 0) > 0
        switch code {
        case 53:
            hide(); return true
        case 13 where cmd:
            hide(); return true
        case 125:
            move(1, extend: shift); return true
        case 45 where ctrl:
            move(1, extend: shift); return true
        case 126:
            move(-1, extend: shift); return true
        case 35 where ctrl:
            move(-1, extend: shift); return true
        case 36, 76:
            runDefault(); return true
        case 8 where cmd && shift:
            copyFiles(); return true
        case 8 where cmd && !textSelected:
            copyPaths(list.selectedRows); return true
        case 16 where cmd:
            toggleQuickLook(); return true
        case 49 where !cmd && !ctrl && (query.isEmpty || window.nativeWindow.firstResponder === list):
            toggleQuickLook(); return true
        case 31 where cmd:
            open(list.selectedRows); return true
        case 15 where cmd && !shift:
            beginRename(); return true
        case 15 where cmd && shift:
            reveal(); return true
        case 51 where cmd:
            forget(); return true
        default:
            return false
        }
    }

    private func move(_ d: Int, extend: Bool) {
        guard !shown.isEmpty else { return }
        if extend {
            list.extendSelection(to: min(max(0, list.selection + d), shown.count - 1))
        } else {
            list.moveSelection(d)
        }
        scrollToSelection()
        refreshQuickLook()
    }

    private var selectedPaths: [String] {
        list.selectedRows.filter { shown.indices.contains($0) }.map { shown[$0].path }
    }

    private func runDefault() {
        switch returnAction {
        case "path": copyPaths(list.selectedRows)
        case "open": open(list.selectedRows)
        default: copyFiles()
        }
    }

    func copyFiles() {
        let paths = selectedPaths
        guard !paths.isEmpty else { NSSound.beep(); return }
        Self.writeFiles(paths, to: .general)
        onCopied?()
        PathShelf.shared.add(paths, why: .clipboard)
        let what = paths.count == 1 ? (paths[0] as NSString).lastPathComponent : "\(paths.count) files"
        window.showToast("Copied \(what) — ⌘V attaches it", symbol: "doc.on.doc.fill", centered: true)
        log?("paths: copied file(s) \(paths.joined(separator: ", "))")
    }

    static func writeFiles(_ paths: [String], to pb: NSPasteboard) {
        pb.clearContents()
        pb.writeObjects(paths.map { p -> NSPasteboardItem in
            let it = NSPasteboardItem()
            it.setString(URL(fileURLWithPath: p).absoluteString, forType: .fileURL)
            it.setString(p, forType: .string)
            return it
        })
    }

    private func copyPaths(_ rows: [Int]) {
        let paths = rows.filter { shown.indices.contains($0) }.map { shown[$0].path }
        guard !paths.isEmpty else { NSSound.beep(); return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(paths.joined(separator: "\n"), forType: .string)
        onCopied?()
        let what = paths.count == 1 ? Self.folder(paths[0]) + "/" + (paths[0] as NSString).lastPathComponent
            : "\(paths.count) paths"
        window.showToast("Copied \(what)", symbol: "checkmark.circle.fill", centered: true)
        log?("paths: copied path(s) \(paths.joined(separator: ", "))")
    }

    private func open(_ rows: [Int]) {
        let paths = rows.filter { shown.indices.contains($0) }.map { shown[$0].path }
        for p in paths { FilePopup.open(p, over: window.nativeWindow) }
    }

    private func reveal() {
        let urls = selectedPaths.map { URL(fileURLWithPath: $0) }
        guard !urls.isEmpty else { return }
        NSWorkspace.shared.activateFileViewerSelecting(urls)
    }

    private func forget() {
        let paths = selectedPaths
        guard !paths.isEmpty else { return }
        PathShelf.shared.remove(paths)
        window.showToast(paths.count == 1 ? "Removed from the list (the file is untouched)"
                         : "Removed \(paths.count) from the list (files untouched)",
                         symbol: "minus.circle", centered: true)
    }

    var renameEditor: NSText? { rename?.textField?.currentEditor() }

    private func beginRename(_ index: Int? = nil) {
        let i = index ?? list.selection
        guard shown.indices.contains(i) else { return }
        commitRename()
        let item = shown[i]
        let name = (item.path as NSString).lastPathComponent
        let r = InlineRename.begin(parent: list, nameRect: list.nameRect(i),
                                   name: name, isDir: false, colors: colors,
                                   onCommit: { [weak self] in self?.commitRename() })
        r.setPath(item.path)
        rename = r
        PopupWindow.transientEscape = { [weak self] in self?.cancelRename() }
        let w = window.nativeWindow
        guard w.makeFirstResponder(r.textField!) else { cancelRename(); return }
    }

    private func endRename() -> (path: String, text: String)? {
        guard let r = rename else { return nil }
        let result = r.end(list: list, window: window.nativeWindow)
        rename = nil
        PopupWindow.transientEscape = nil
        return result
    }

    func cancelRename() {
        rename?.cancel(window: window.nativeWindow)
        rename = nil
        PopupWindow.transientEscape = nil
    }

    private func commitRename() {
        guard let (path, text) = endRename() else { return }
        let old = (path as NSString).lastPathComponent
        let new = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !new.isEmpty, new != old else { return }
        guard !new.contains("/"), new != ".", new != ".." else {
            window.showToast("can't rename: \"\(new)\" is not a valid name", symbol: "exclamationmark.triangle", centered: true)
            return
        }
        let dst = ((path as NSString).deletingLastPathComponent as NSString).appendingPathComponent(new)
        if new.lowercased() != old.lowercased(), FileManager.default.fileExists(atPath: dst) {
            window.showToast("can't rename: \"\(new)\" already exists", symbol: "exclamationmark.triangle", centered: true)
            return
        }
        do {
            try FileManager.default.moveItem(atPath: path, toPath: dst)
        } catch {
            window.showToast("rename failed: \(error.localizedDescription)", symbol: "exclamationmark.triangle", centered: true)
            return
        }
        FileDrag.onFileOp?(path, dst)
        FileOps.recordRename(from: path, to: dst)
        PathShelf.shared.renamed(from: path, to: dst)
        reload(keepSelection: true)
        if let i = shown.firstIndex(where: { $0.path == dst }) {
            list.selection = i
            scrollToSelection()
        }
        window.showToast("renamed \"\(old)\" to \"\(new)\"", symbol: "checkmark.circle.fill", centered: true)
        log?("paths: renamed \"\(old)\" to \"\(new)\"")
    }

    private var quickLookUp: Bool {
        QLPreviewPanel.sharedPreviewPanelExists() && QLPreviewPanel.shared().isVisible
            && QLPreviewPanel.shared().dataSource === self
    }

    private func toggleQuickLook() {
        guard let panel = QLPreviewPanel.shared() else { return }
        if quickLookUp {
            panel.orderOut(nil)
            window.nativeWindow.makeKey()
            return
        }
        quickLookPaths = selectedPaths
        guard !quickLookPaths.isEmpty else { return }
        panel.dataSource = self
        panel.delegate = self
        panel.reloadData()
        panel.currentPreviewItemIndex = 0
        panel.makeKeyAndOrderFront(nil)
    }

    private func refreshQuickLook() {
        guard quickLookUp else { return }
        quickLookPaths = selectedPaths
        QLPreviewPanel.shared().reloadData()
    }

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { quickLookPaths.count }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! {
        guard quickLookPaths.indices.contains(index) else { return nil }
        return URL(fileURLWithPath: quickLookPaths[index]) as NSURL
    }
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown else { return false }
        switch event.keyCode {
        case 49, 53: toggleQuickLook(); return true
        case 125: move(1, extend: false); return true
        case 126: move(-1, extend: false); return true
        default: return false
        }
    }

    var testState: [String: Any] {
        ["shown": window.isShown, "key": window.nativeWindow.isKeyWindow,
         "level": window.nativeWindow.level.rawValue,
         "selection": list.selection, "query": query,
         "rows": shown.map { ["path": $0.path, "why": $0.why.rawValue] },
         "frame": { () -> [Int] in let f = window.nativeWindow.frame
             return [f.origin.x, f.origin.y, f.width, f.height].map { Int($0.rounded()) } }()]
    }

    func testSelect(_ i: Int) {
        guard shown.indices.contains(i) else { return }
        list.selection = i
    }

    func testReturn() { runDefault() }
}
