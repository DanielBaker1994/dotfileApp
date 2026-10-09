import AppKit
import Quartz

// MARK: - Compare view (a shared-window view): Beyond Compare-style Text Compare
//
// PRD-compare.md. Phase 1 = Text Compare: two files, pasted texts, or one of
// each, side by side, lined up (CompareText.swift = the engine, ComparePane
// .swift = the drawn panes). Sessions are pills; "+" = the start page (two
// path fields + Recent). It lives in the shared window like notes / AI and is
// dismissed exactly like them (✕, Cmd+W, Hyper+N, focus loss, Ctrl+Tab,
// Esc only with its "Esc Hides Window" on). `.compareText` = a Text Compare
// pushed ON the view (a back step, the way jira detail sits on jira): Folder
// Compare (phase 2) opens file pairs there; `do:compare:open-sub` today.
// Config: commands.toml [compare]. Opened from the Hyper+S palette
// (/compare), the header icon, the file browser / /paths right-click
// ("Select for Compare", "Compare to …"), the CLI (`kitchen-sink
// compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]`, git difftool).

func compareEnabled() -> Bool {
    tri(configSectionValue("compare", "enabled")) == true
}

// [compare], read once per open / show (configSectionValue reads the file)
struct CompareConfig {
    var entries: [String: String] = [:]

    static func load() -> CompareConfig {
        var c = CompareConfig()
        if let text = readConfigText() {
            for e in configSectionEntries(configLines(text), "compare") { c.entries[e.key] = e.value }
        }
        return c
    }

    func string(_ k: String, _ d: String) -> String {
        let v = entries[k]?.trimmingCharacters(in: .whitespaces) ?? ""
        return v.isEmpty ? d : v
    }
    func number(_ k: String, _ d: Double) -> Double { entries[k].flatMap { Double($0.trimmingCharacters(in: .whitespaces)) } ?? d }
    func bool(_ k: String, _ d: Bool) -> Bool { tri(entries[k]) ?? d }
    // a user-facing string ([compare] *-label); `{}` = the variable part
    func label(_ k: String, _ d: String, _ arg: String = "") -> String {
        string(k + "-label", d).replacingOccurrences(of: "{}", with: arg)
    }

    var importance: Importance {
        Importance(leadingWS: bool("ignore-leading-ws", true), trailingWS: bool("ignore-trailing-ws", true),
                   embeddedWS: bool("ignore-embedded-ws", false), ignoreCase: bool("ignore-case", false),
                   lineEndings: bool("ignore-line-endings", true), blankLines: bool("ignore-blank-lines", false))
    }
    var contextLines: Int { max(0, Int(number("context-lines", 3))) }
    var tabWidth: Int { max(1, min(16, Int(number("tab-width", 4)))) }
    var maxLines: Int { max(1000, Int(number("max-lines", 200_000))) }
    var recentLimit: Int { max(0, min(500, Int(number("recent", 30)))) }
    var gutterArrows: String {
        let v = string("gutter-arrows", "hover").lowercased()
        return ["hover", "always", "off"].contains(v) ? v : "hover"
    }
    // the panes' font: [compare] font / font-size, else the notes font
    var font: NSFont {
        let notes = (try? String(contentsOfFile: settings.commandsConfPath, encoding: .utf8)).map(configLines)
            .map { configSectionEntries($0, "notes") } ?? []
        let size = CGFloat(number("font-size", Double(notes.first { $0.key == "font-size" }?.value ?? "") ?? 13))
        let name = string("font", notes.first { $0.key == "font" }?.value ?? "")
        let f = name.isEmpty ? nil : NSFont(name: name, size: size)
        return (f.map { $0.isFixedPitch ? $0 : nil } ?? nil) ?? NSFont.monospacedSystemFont(ofSize: max(8, min(40, size)), weight: .regular)
    }
}

let compareTint = NSColor(srgbRed: 0.53, green: 0.75, blue: 0.95, alpha: 1)
let compareAppIcon: NSImage = {
    let p = (CompareConfig.load().string("icon", "") as NSString).expandingTildeInPath
    return (p.isEmpty ? nil : fileIconTile(p, size: appIconSize))
        ?? glyphIcon("arrow.left.arrow.right", fallback: "⇆", tint: compareTint)
}()
let compareNavIcon: NSImage = {
    let p = (CompareConfig.load().string("icon", "") as NSString).expandingTildeInPath
    return (p.isEmpty ? nil : NSImage(contentsOfFile: p))
        ?? glyphIcon("arrow.left.arrow.right", fallback: "⇆", tint: compareTint, size: 32, tile: false)
}()

// MARK: - Recent pairs (~/.cache/kitchen-sink/compare-recent.json)

struct CompareRecentEntry: Equatable {
    var left: String
    var right: String
    var used: Date
}

enum CompareRecent {
    static var path: String { NSHomeDirectory() + "/.cache/kitchen-sink/compare-recent.json" }
    // snapshots of pasted sides (a Recent row opens them like files)
    static var pastedDir: String { NSHomeDirectory() + "/.cache/kitchen-sink/compare-pasted" }
    static func isPasted(_ p: String) -> Bool { p.hasPrefix(pastedDir + "/") }

    static func load() -> [CompareRecentEntry] {
        guard let d = FileManager.default.contents(atPath: path),
              let arr = (try? JSONSerialization.jsonObject(with: d)) as? [[String: Any]] else { return [] }
        return arr.compactMap { o in
            guard let l = o["left"] as? String, let r = o["right"] as? String else { return nil }
            return CompareRecentEntry(left: l, right: r, used: Date(timeIntervalSince1970: o["used"] as? Double ?? 0))
        }
    }

    static func add(_ left: String, _ right: String, limit: Int) {
        guard limit > 0 else { return }
        var all = load().filter { !($0.left == left && $0.right == right) }
        all.insert(CompareRecentEntry(left: left, right: right, used: Date()), at: 0)
        save(Array(all.prefix(limit)))
    }

    static func remove(_ e: CompareRecentEntry) { save(load().filter { $0 != e }) }

    static func clearAll() {
        try? FileManager.default.removeItem(atPath: pastedDir)
        try? FileManager.default.createDirectory(atPath: pastedDir, withIntermediateDirectories: true)
        try? Data("[]".utf8).write(to: URL(fileURLWithPath: path))
    }

    static func save(_ all: [CompareRecentEntry]) {
        // a snapshot nothing lists any more goes away
        let keep = Set(all.flatMap { [$0.left, $0.right] })
        for f in (try? FileManager.default.contentsOfDirectory(atPath: pastedDir)) ?? [] where !keep.contains(pastedDir + "/" + f) {
            try? FileManager.default.removeItem(atPath: pastedDir + "/" + f)
        }
        let arr = all.map { ["left": $0.left, "right": $0.right, "kind": "text", "used": $0.used.timeIntervalSince1970] as [String: Any] }
        guard let d = try? JSONSerialization.data(withJSONObject: arr, options: [.prettyPrinted]) else { return }
        try? FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        try? d.write(to: URL(fileURLWithPath: path), options: .atomic)
    }

    static func age(_ d: Date) -> String {
        let s = Date().timeIntervalSince(d)
        if s < 60 { return "just now" }
        if s < 3600 { return "\(Int(s / 60)) min ago" }
        if s < 86400 { return "\(Int(s / 3600)) h ago" }
        if s < 172_800 { return "yesterday" }
        let f = DateFormatter()
        f.dateFormat = s < 7 * 86400 ? "EEE" : "yyyy-MM-dd"
        return f.string(from: d)
    }
}

// the start page's Recent list, drawn (cursor pill + accent edge like every list)
final class CompareRecentList: NSView {
    // a pane (Ctrl+H/J/K/L): the window's keys drive it while it has focus
    override var acceptsFirstResponder: Bool { true }
    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    var rows: [CompareRecentEntry] = [] {
        didSet { selection = min(selection, max(0, rows.count - 1)); needsDisplay = true; refreshMissing() }
    }
    var selection = 0 { didSet { needsDisplay = true } }
    var onOpen: ((Int) -> Void)?
    var menuFor: ((Int) -> NSMenu?)?
    var onRemove: ((Int) -> Void)?       // the row's ✕ (shown on hover)
    let rowH: CGFloat = 28
    private var hover: Int?
    private var hoverX = false
    private func xRect(_ i: Int) -> NSRect {
        NSRect(x: bounds.width - 30, y: CGFloat(i) * rowH + (rowH - 20) / 2, width: 20, height: 20)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }
    override func mouseMoved(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let i = Int(p.y / rowH)
        let h = rows.indices.contains(i) ? i : nil
        let x = h.map { xRect($0).insetBy(dx: -3, dy: -3).contains(p) } ?? false
        if h != hover || x != hoverX { hover = h; hoverX = x; needsDisplay = true }
    }
    override func mouseExited(with e: NSEvent) { hover = nil; hoverX = false; needsDisplay = true }
    override var isFlipped: Bool { true }

    // row index → which sides point at a file that no longer exists
    private var missingCache: [Int: (left: Bool, right: Bool)] = [:]
    private func tilde(_ p: String) -> String { (p as NSString).abbreviatingWithTildeInPath }
    func isMissing(_ i: Int) -> Bool { missingCache[i].map { $0.left || $0.right } ?? false }
    func missingPaths(_ i: Int) -> [String] {
        guard rows.indices.contains(i), let m = missingCache[i] else { return [] }
        return (m.left ? [rows[i].left] : []) + (m.right && rows[i].right != rows[i].left ? [rows[i].right] : [])
    }

    // re-checked whenever the rows are set and every time the start page shows
    func refreshMissing() {
        missingCache.removeAll()
        let fm = FileManager.default
        for (i, e) in rows.enumerated() {
            missingCache[i] = (!fm.fileExists(atPath: e.left), !fm.fileExists(atPath: e.right))
        }
        removeAllToolTips()
        for i in rows.indices where isMissing(i) {
            addToolTip(NSRect(x: 0, y: CGFloat(i) * rowH, width: max(bounds.width, 2000), height: rowH),
                       owner: self, userData: nil)
        }
        needsDisplay = true
    }
    @objc func view(_ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint,
                    userData data: UnsafeMutableRawPointer?) -> String {
        let i = Int(point.y / rowH)
        let gone = missingPaths(i)
        return gone.isEmpty ? "" : "Can't open: no longer exists\n" + gone.map(tilde).joined(separator: "\n")
    }

    override func draw(_ dirty: NSRect) {
        let font = NSFont.systemFont(ofSize: 12.5, weight: .medium), small = NSFont.systemFont(ofSize: 11.5)
        let warnFont = NSFont.systemFont(ofSize: 11, weight: .semibold)
        for (i, e) in rows.enumerated() {
            let r = NSRect(x: 0, y: CGFloat(i) * rowH, width: bounds.width, height: rowH)
            guard r.intersects(dirty) else { continue }
            let gone = missingCache[i] ?? (false, false)
            let missing = gone.left || gone.right
            if i == selection {
                let pill = NSBezierPath(roundedRect: r.insetBy(dx: 2, dy: 2), xRadius: 6, yRadius: 6)
                colors.highlight.withAlphaComponent(0.6).setFill()
                pill.fill()
                colors.accentOn.setFill()
                NSRect(x: r.minX + 2, y: r.minY + 5, width: 3, height: r.height - 10).fill()
            }
            let pl = CompareRecent.isPasted(e.left), pr = CompareRecent.isPasted(e.right)
            let ln = pl ? "pasted text" : (e.left as NSString).lastPathComponent
            let rn = pr ? "pasted text" : (e.right as NSString).lastPathComponent
            let name = ln == rn ? ln : "\(ln) ⇆ \(rn)"
            let y = r.minY + 6
            // a pair with a vanished side: ⚠ before the name, the name dimmed,
            // the missing side's folder struck through in the danger hue
            var nameX: CGFloat = 14
            if missing {
                ButtonStyle.symbol("exclamationmark.triangle.fill", in: NSRect(x: 12, y: r.minY, width: 16, height: r.height),
                                   color: colors.tone(.danger), size: 11)
                nameX = 32
            }
            let nameAttrs: [NSAttributedString.Key: Any] = [
                .font: font,
                .foregroundColor: missing ? colors.dim : colors.text
            ]
            let np = NSMutableParagraphStyle()
            np.lineBreakMode = .byTruncatingMiddle
            var na = nameAttrs
            na[.paragraphStyle] = np
            (name as NSString).draw(in: NSRect(x: nameX, y: y, width: 234 - nameX, height: 18), withAttributes: na)
            let p = NSMutableParagraphStyle()
            p.lineBreakMode = .byTruncatingMiddle
            let base: [NSAttributedString.Key: Any] = [.font: small, .foregroundColor: colors.dim, .paragraphStyle: p]
            var goneAttrs = base
            goneAttrs[.foregroundColor] = colors.dim.withAlphaComponent(0.6)
            goneAttrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
            goneAttrs[.strikethroughColor] = colors.tone(.danger).withAlphaComponent(0.7)
            let dl = pl ? "(pasted)" : tilde((e.left as NSString).deletingLastPathComponent)
            let dr = pr ? "(pasted)" : tilde((e.right as NSString).deletingLastPathComponent)
            let dirs = NSMutableAttributedString()
            if dl == dr && gone.left == gone.right {
                // both in one folder: say it once
                dirs.append(NSAttributedString(string: dl, attributes: gone.left ? goneAttrs : base))
            } else {
                dirs.append(NSAttributedString(string: dl, attributes: gone.left ? goneAttrs : base))
                dirs.append(NSAttributedString(string: "  ⇆  ", attributes: base))
                dirs.append(NSAttributedString(string: dr, attributes: gone.right ? goneAttrs : base))
            }
            let age = CompareRecent.age(e.used) as NSString
            let attrs: [NSAttributedString.Key: Any] = [.font: small, .foregroundColor: colors.dim]
            let ageW = age.size(withAttributes: attrs).width
            let rightEdge = bounds.width - 12 - (hover == i && onRemove != nil ? 28 : 0)
            var ageX = rightEdge - ageW
            if missing {
                let tag = "missing" as NSString
                let tagAttrs: [NSAttributedString.Key: Any] = [.font: warnFont, .foregroundColor: colors.tone(.danger)]
                // "missing" at the right edge, the age just left of it
                let tw = tag.size(withAttributes: tagAttrs).width
                tag.draw(at: NSPoint(x: rightEdge - tw, y: y + 1), withAttributes: tagAttrs)
                ageX -= tw + 10
            }
            // the folders take what's left of the age / tag cluster
            dirs.draw(in: NSRect(x: 240, y: y + 1, width: max(40, ageX - 14 - 240), height: 18))
            // the ✕ takes the right end on hover (age / "missing" step left)
            if hover == i, onRemove != nil {
                let xr = xRect(i)
                if hoverX {
                    colors.tone(.danger).withAlphaComponent(0.2).setFill()
                    NSBezierPath(ovalIn: xr).fill()
                }
                ButtonStyle.cross(in: xr, color: hoverX ? colors.tone(.danger) : colors.dim, arm: 3.5)
            }
            age.draw(at: NSPoint(x: ageX, y: y + 1), withAttributes: attrs)
        }
    }
    override func mouseDown(with e: NSEvent) {
        let i = Int(convert(e.locationInWindow, from: nil).y / rowH)
        guard rows.indices.contains(i) else { return }
        if onRemove != nil, xRect(i).insetBy(dx: -3, dy: -3).contains(convert(e.locationInWindow, from: nil)) {
            onRemove?(i)
            return
        }
        selection = i
        // one click opens (a start page's list of links); a missing pair
        // explains itself instead of doing nothing
        if e.clickCount == 1 { onOpen?(i) }
    }
    override func menu(for event: NSEvent) -> NSMenu? {
        let i = Int(convert(event.locationInWindow, from: nil).y / rowH)
        guard rows.indices.contains(i) else { return nil }
        selection = i
        return menuFor?(i)
    }
}

// a side's path above its pane (click = edit it: Cmd+L)
final class CompareHeaderLabel: NSView {
    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    var text = "" { didSet { needsDisplay = true } }
    var dirty = false { didSet { needsDisplay = true } }
    var focused = false { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?
    override var isFlipped: Bool { true }
    override func draw(_ dirty: NSRect) {
        let p = NSMutableParagraphStyle()
        p.lineBreakMode = .byTruncatingHead
        let font = NSFont.monospacedSystemFont(ofSize: 11.5, weight: focused ? .semibold : .regular)
        let s = NSMutableAttributedString(string: text, attributes: [.font: font, .paragraphStyle: p,
                                                                     .foregroundColor: focused ? colors.text : colors.dim])
        if self.dirty {
            s.append(NSAttributedString(string: "  ● edited", attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold),
                                                                          .foregroundColor: colors.tone(.warning)]))
        }
        s.draw(in: NSRect(x: 10, y: (bounds.height - 16) / 2, width: bounds.width - 20, height: 16))
        if focused {
            colors.accentOn.setFill()
            NSRect(x: 0, y: bounds.height - 2, width: bounds.width, height: 2).fill()
        }
    }
    override func mouseDown(with event: NSEvent) { onClick?() }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
}

// MARK: - the window

final class CompareWindow: CardWindowController, ComparePaneHost, FolderHost, NSTextViewDelegate, NSTextFieldDelegate {
    private static var live: CompareWindow?
    private static var subLive: CompareWindow?
    static var current: CompareWindow? { live }
    // `.compareText`: a Text Compare pushed on the view (back = the view)
    static var sub: CompareWindow? { subLive }

    let isSub: Bool
    private var colors = cardColors("compare")
    private var cfg = CompareConfig.load()

    // sessions (pills); nil selection = the start page
    private(set) var sessions: [CompareSession] = []
    private var selected: Int?
    // the Ctrl+B W view switcher's "where": the session shown, or Home
    var whereText: String { session?.label ?? "Home" }
    var session: CompareSession? { selected.flatMap { sessions.indices.contains($0) ? sessions[$0] : nil } }

    // chrome
    private var root: ConfPane!
    private var pills: PopupTabsBar!
    private var sidebarWide: CGFloat = 0
    // the icon rail (⌘\\) when collapsed
    private var sidebarW: CGFloat {
        get { pills?.width(expanded: sidebarWide) ?? sidebarWide }
        set { sidebarWide = newValue }
    }
    // start page
    private let start = ConfPane()
    private let leftBox = JiraInputBox(placeholder: "Left: a file or folder path (Tab completes)")
    private let rightBox = JiraInputBox(placeholder: "Right: a file or folder path")
    private let leftLabel = NSTextField(labelWithString: "Left")
    private let rightLabel = NSTextField(labelWithString: "Right")
    private var browseL: ThemeButton!
    private var browseR: ThemeButton!
    // Compare ⏎ | Compare Pasted Text… in one capsule; Recent's own pair
    private var startActions: CapsuleButtons!
    private var recentActions: CapsuleButtons!
    private var suggestActions: CapsuleButtons!
    private let suggestTitle = NSTextField(labelWithString: "READY")
    private let recentTitle = NSTextField(labelWithString: "RECENT")
    private let recentFilter = JiraInputBox(placeholder: "filter")
    private let recentList = CompareRecentList()
    private let recentScroll = NSScrollView()
    private let startHint = NSTextField(wrappingLabelWithString: "")
    private var recentAll: [CompareRecentEntry] = []
    // text compare
    private let body = ConfPane()
    private lazy var folderPage = FolderPage(host: self)
    private let toolbar = ConfPane()
    private let filterSeg = ConfSegmented(CompareFilter.allCases.map(\.title))
    private let summary = NSTextField(labelWithString: "")
    private var toolButtons: [ThemeButton] = []
    private let findBox = JiraInputBox(placeholder: "find")
    private var findMode = FindMode.none
    private enum FindMode { case none, find, goTo }
    private let headerL = CompareHeaderLabel(), headerR = CompareHeaderLabel()
    private let pathEdit = JiraInputBox(placeholder: "path — Return opens it into this side, Tab completes")
    private var pathEditSide: CompareSide?
    private let banner = ConfPane()
    private let bannerText = NSTextField(labelWithString: "")
    private let bannerReload = ThemedPushButton(title: "Reload", target: nil, action: nil)
    private let bannerKeep = ThemedPushButton(title: "Keep Mine", target: nil, action: nil)
    private var bannerSide: CompareSide?
    private let thumb = CompareThumbnail()
    private let scroll = NSScrollView()
    private let pane = ComparePaneView()
    private let details = CompareDetails()
    private let status = NSTextField(labelWithString: "")
    private let emptyHint = NSTextField(labelWithString: "")
    private var editor: CompareEditor?
    private var showDetails = UserDefaults.standard.object(forKey: "compareDetails") as? Bool ?? true
    private var showWhitespace = UserDefaults.standard.bool(forKey: "compareWhitespace")
    // Align With: the first line picked (side + line); the other side's pick aligns
    private var alignPick: (side: CompareSide, line: Int)?
    private var targets: [ClosureTarget] = []
    private var diffGen = 0
    private var diffRunning = false

    // MARK: open

    static func discard() { live = nil }

    static func create(controller: SwitcherController, frame: NSRect?) {
        guard live == nil else { return }
        live = CompareWindow(controller: controller, frame: frame, sub: false)
    }

    static func createSub(controller: SwitcherController, frame: NSRect?) -> CompareWindow {
        if let s = subLive { return s }
        let s = CompareWindow(controller: controller, frame: frame, sub: true)
        subLive = s
        return s
    }

    func showStandalone() {
        NSApp.activate(ignoringOtherApps: true)
        if !window.isVisible { window.center() }
        window.makeKeyAndOrderFront(nil)
        didShow()
    }

    private init(controller: SwitcherController, frame: NSRect?, sub: Bool) {
        isSub = sub
        let c = CompareConfig.load()
        let f = frame ?? NSRect(x: 0, y: 0, width: c.number("width", 1200), height: c.number("height", 760))
        super.init(controller: controller, frame: f, title: sub ? "Text Compare" : "Compare",
                   minSize: NSSize(width: 640, height: 380))
        PopupThemeDefaults.colors = colors
        window.contentView = themedRoot(buildContent(), name: "compare", colors: colors,
                                        headerColor: cardHeaderColor("compare"),
                                        icon: compareAppIcon, title: sub ? "Text Compare" : "Compare")
        if frame != nil { window.setFrame(f, display: false) }
        applyConfig()
        showPage()
        if !sub {
            restoreSessions()
            NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { [weak self] _ in
                self?.persistNow()
            }
        }
    }

    // the shared window brought the view back
    override func didShow() {
        cfg = CompareConfig.load()
        applyConfig()
        if session == nil { reloadRecent() }
        if editor == nil {
            if session != nil, !(window.firstResponder is NSText) { window.makeFirstResponder(pane) }
            else if session == nil, !(window.firstResponder is NSText) { window.makeFirstResponder(leftBox.field) }
        }
        syncAll()
    }

    // hidden (whole window: stopVoice) or parked for another view: the
    // section editor commits (nothing typed is ever dropped). A whole-window
    // hide = "done" for `compare --wait` (git difftool).
    override func slotPark(stopVoice: Bool) {
        commitEditor()
        super.slotPark(stopVoice: stopVoice)
        if stopVoice {
            finishWaiters()
            // hidden from the pushed Text Compare: the view under it (a git
            // --dir-diff folder session) is done too
            if isSub { CompareWindow.current?.finishWaiters() }
        }
        persistNow()
    }

    func windowWillClose(_ notification: Notification) { finishWaiters() }

    func finishWaiters() {
        for s in sessions where !s.waiters.isEmpty {
            let w = s.waiters
            s.waiters = []
            w.forEach { $0() }
            // git deletes its temp files now: a clean git session goes with them
            if s.git && !s.isDirty, let i = sessions.firstIndex(where: { $0 === s }) { removeSession(i) }
        }
    }

    private func applyConfig() {
        pane.setFont(cfg.font)
        pane.needsDisplay = true
    }

    // MARK: build

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

    private func buildContent() -> NSView {
        let r = ConfPane()
        root = r
        var pcfg = PopupConfig(name: "compare-sessions")
        pcfg.colors = colors
        // sidebar mode has permanent Home + New rows instead of a "+"
        pcfg.tabsAddButton = !isSub && (configSectionValue("compare", "sidebar-width").flatMap { Double($0) } ?? 210) <= 0
        pills = PopupTabsBar(config: pcfg)
        pills.closable = true
        pills.onSelect = { [weak self] i in self?.select(i) }
        pills.onClick = { [weak self] i in if self?.selected == nil { self?.select(i) } }
        pills.onAddTab = { [weak self] in self?.showStartPage() }
        pills.onCloseTab = { [weak self] i in self?.closeSession(i) }
        pills.menuFor = { [weak self] i in self?.pillMenu(i) }
        pills.pathTip = { [weak self] i in
            guard let self, self.sessions.indices.contains(i) else { return nil }
            let s = self.sessions[i]
            return "\(s.path[.left] ?? s.name(.left))\n\(s.path[.right] ?? s.name(.right))"
        }
        pills.isHidden = isSub
        // the sessions as a left sidebar (`[compare] sidebar-width`, 0 = pills);
        // "+" beside the label = the start page; its edge drags to resize
        sidebarW = isSub ? 0 : (configSectionValue("compare", "sidebar-width").flatMap { Double($0) }.map { CGFloat($0) } ?? 210)
        if sidebarW > 0 {
            pills.vertical = true
            pills.sectionTitle = "Open"
            pills.collapseKey = "compare"
            // Home (the start page: recent pairs, open files / folders) and
            // New (two empty sides) never move: the PINNED rows on top
            pills.pinnedTitle = "Compare"
            pills.pinned = ["home", "new"]
            pills.maxPinnedShown = 2
            pills.pinnedIconFor = { $0 == "home" ? "house" : "square.and.pencil" }
            pills.pinnedLabel = { $0 == "home" ? "Home" : "New text compare" }
            pills.pinnedTip = { $0 == "home" ? "Recent comparisons, open files or folders (⌘0)" : "Two empty sides — click one and paste (⌘N)" }
            pills.onPinned = { [weak self] id in
                if id == "home" { self?.showStartPage() } else { self?.startPasted() }
            }
            pills.rowIcon = { [weak self] i in
                guard let self, self.sessions.indices.contains(i) else { return nil }
                return self.sessions[i].folder != nil ? "folder" : "arrow.left.arrow.right"
            }
            pills.onWidthChange = { [weak self, weak r] w, done in
                guard let self else { return }
                self.sidebarW = w.rounded()
                r?.needsLayout = true
                if done { saveConfigValue(section: "compare", key: "sidebar-width", value: String(Int(self.sidebarW))) }
            }
        }
        r.addSubview(pills)
        buildStart()
        buildText()
        r.addSubview(start)
        r.addSubview(body)
        r.addSubview(folderPage.root)
        folderPage.root.isHidden = true
        r.onLayout = { [weak self] b in self?.layoutAll(b) }
        return r
    }

    private func buildStart() {
        for (l, box) in [(leftLabel, leftBox), (rightLabel, rightBox)] {
            label(l, size: 12, weight: .semibold, color: colors.text)
            start.addSubview(l)
            box.field.delegate = self
            start.addSubview(box)
        }
        browseL = button("folder", "Choose the left file or folder…") { [weak self] in self?.browse(into: self?.leftBox) }
        browseR = button("folder", "Choose the right file or folder…") { [weak self] in self?.browse(into: self?.rightBox) }
        start.addSubview(browseL)
        start.addSubview(browseR)
        startActions = CapsuleButtons([
            .init(title: "Compare  ⏎", symbol: "arrow.left.arrow.right", primary: true) { [weak self] in self?.compareFromStart() },
            .init(title: "New Text Compare", symbol: "square.and.pencil", primary: false) { [weak self] in self?.startPasted() },
        ])
        startActions.colors = colors
        startActions.toolTip = "Compare: files on both sides → Text Compare, folders → Folder Compare. "
            + "New Text Compare: two empty sides — click one and paste (⌘V)."
        start.addSubview(startActions)
        label(suggestTitle, size: 11, weight: .bold)
        start.addSubview(suggestTitle)
        suggestActions = CapsuleButtons([])
        suggestActions.colors = colors
        suggestActions.toolTip = "What you were just doing: one click fills a side (or opens the pair)"
        start.addSubview(suggestActions)
        label(recentTitle, size: 11, weight: .bold)
        start.addSubview(recentTitle)
        recentActions = CapsuleButtons([])
        recentActions.colors = colors
        start.addSubview(recentActions)
        recentFilter.field.delegate = self
        start.addSubview(recentFilter)
        recentList.colors = colors
        recentList.onOpen = { [weak self] i in self?.openRecent(i) }
        recentList.menuFor = { [weak self] i in self?.recentMenu(i) }
        recentList.onRemove = { [weak self] i in
            guard let self, self.recentList.rows.indices.contains(i) else { return }
            CompareRecent.remove(self.recentList.rows[i])
            self.reloadRecent()
        }
        recentScroll.documentView = recentList
        recentScroll.drawsBackground = false
        recentScroll.hasVerticalScroller = true
        recentScroll.autohidesScrollers = true
        recentScroll.scrollerStyle = .overlay
        start.addSubview(recentScroll)
        label(startHint, size: 12)
        startHint.lineBreakMode = .byWordWrapping
        start.addSubview(startHint)
        start.onLayout = { [weak self] b in self?.layoutStart(b) }
    }

    private func buildText() {
        toolbar.fill = colors.mantle
        filterSeg.colors = colors
        filterSeg.tips = ["Every line (⌘1)", "Only differences (⌘2)", "Only lines that are the same (⌘3)",
                          "Differences with [compare] context-lines around them (⌘4)"]
        filterSeg.onChange = { [weak self] i in self?.setFilter(CompareFilter.allCases[i]) }
        toolbar.addSubview(filterSeg)
        label(summary, size: 12, weight: .semibold, color: colors.text)
        toolbar.addSubview(summary)
        toolButtons = [
            button("chevron.up", "Previous difference (⌃P)") { [weak self] in self?.jumpSection(-1) },
            button("chevron.down", "Next difference (⌃N)") { [weak self] in self?.jumpSection(1) },
            button("arrow.left.arrow.right", "Swap sides (⌘⌥X)") { [weak self] in self?.swapSides() },
            button("arrow.clockwise", "Reload both files and compare again (F5)") { [weak self] in self?.reload() },
            button("slider.horizontal.3", "Importance: what counts as a difference") { [weak self] in self?.showImportanceMenu() },
            button("magnifyingglass", "Find (⌘F)") { [weak self] in self?.openFind(.find) },
        ]
        toolButtons.forEach(toolbar.addSubview)
        findBox.field.delegate = self
        findBox.isHidden = true
        toolbar.addSubview(findBox)
        body.addSubview(toolbar)

        for (h, side) in [(headerL, CompareSide.left), (headerR, .right)] {
            h.colors = colors
            h.onClick = { [weak self] in self?.beginPathEdit(side) }
            h.toolTip = "Click (⌘L) to type a path for this side"
            body.addSubview(h)
        }
        pathEdit.field.delegate = self
        pathEdit.isHidden = true
        body.addSubview(pathEdit)

        banner.fill = colors.tone(.warning).withAlphaComponent(0.18)
        label(bannerText, size: 12, weight: .semibold, color: colors.text)
        banner.addSubview(bannerText)
        for (b, keep) in [(bannerReload, false), (bannerKeep, true)] {
            let t = ClosureTarget { [weak self] in self?.resolveBanner(keep: keep) }
            targets.append(t)
            b.target = t
            b.action = #selector(ClosureTarget.run)
            b.controlSize = .small
            banner.addSubview(b)
        }
        bannerReload.role = .primary
        banner.isHidden = true
        body.addSubview(banner)

        thumb.host = self
        thumb.onScroll = { [weak self] f in self?.scrollToFraction(f) }
        thumb.toolTip = "Every difference in the file — click or drag to go there"
        body.addSubview(thumb)
        pane.host = self
        scroll.documentView = pane
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: scroll.contentView,
                                               queue: .main) { [weak self] _ in self?.paneScrolled() }
        body.addSubview(scroll)
        label(emptyHint, size: 12.5)
        emptyHint.alignment = .center
        emptyHint.isHidden = true
        body.addSubview(emptyHint)
        details.host = self
        details.pane = pane
        body.addSubview(details)
        label(status, size: 11.5)
        status.lineBreakMode = .byTruncatingMiddle
        body.addSubview(status)
        body.onLayout = { [weak self] b in self?.layoutText(b) }
    }

    // MARK: layout

    private func layoutAll(_ b: NSRect) {
        var top: CGFloat = 0
        if !isSub, pills.vertical {
            pills.frame = NSRect(x: 0, y: 0, width: sidebarW, height: b.height)
            let rest = NSRect(x: sidebarW + 4, y: 0, width: max(300, b.width - sidebarW - 4), height: b.height)
            start.frame = rest
            body.frame = rest
            folderPage.root.frame = rest
            return
        }
        if !isSub {
            let h = pills.heightNeeded(forWidth: b.width) + 6
            pills.frame = NSRect(x: 0, y: 4, width: b.width, height: h)
            top = pills.frame.maxY
        }
        let rest = NSRect(x: 0, y: top, width: b.width, height: b.height - top)
        start.frame = rest
        body.frame = rest
        folderPage.root.frame = rest
    }

    private func layoutStart(_ b: NSRect) {
        let pad: CGFloat = 24, lw: CGFloat = 46, bw: CGFloat = 30
        let w = min(b.width - pad * 2, 900)
        let x = (b.width - w) / 2
        var y: CGFloat = 26
        // Left / Right: the label, the folder button (left: closest to
        // reach), then the path field
        for (l, box, br) in [(leftLabel, leftBox, browseL!), (rightLabel, rightBox, browseR!)] {
            l.frame = NSRect(x: x, y: y + 5, width: lw, height: 18)
            br.frame = NSRect(x: x + lw, y: y, width: bw, height: JiraTheme.height + 2)
            box.frame = NSRect(x: x + lw + bw + 6, y: y, width: w - lw - bw - 6, height: JiraTheme.height + 2)
            y += JiraTheme.height + 12
        }
        let aw = startActions.intrinsicContentSize.width
        startActions.frame = NSRect(x: x + (w - aw) / 2 + lw / 2, y: y - 2, width: aw, height: 34)
        y += 42
        startHint.frame = NSRect(x: x, y: y, width: w, height: startHint.stringValue.isEmpty ? 0 : 34)
        y += startHint.stringValue.isEmpty ? 6 : 40
        let sg = suggestActions.items.isEmpty
        suggestTitle.isHidden = sg
        suggestTitle.frame = NSRect(x: x, y: y + 6, width: 64, height: 16)
        suggestActions.frame = NSRect(x: x + 66, y: y - 2, width: sg ? 0 : min(w - 66, suggestActions.intrinsicContentSize.width), height: 34)
        if !sg { y += 42 }
        recentTitle.frame = NSRect(x: x, y: y + 5, width: 64, height: 16)
        let rw = recentActions.intrinsicContentSize.width
        recentActions.frame = NSRect(x: x + 66, y: y - 4, width: recentActions.items.isEmpty ? 0 : rw, height: 30)
        recentFilter.frame = NSRect(x: x + w - 220, y: y, width: 220, height: JiraTheme.height)
        y += JiraTheme.height + 8
        recentScroll.frame = NSRect(x: x, y: y, width: w, height: max(40, b.height - y - 14))
        recentList.frame = NSRect(x: 0, y: 0, width: recentScroll.contentSize.width,
                                  height: max(recentScroll.contentSize.height, CGFloat(recentList.rows.count) * recentList.rowH))
    }

    private func layoutText(_ b: NSRect) {
        let tbH: CGFloat = 36
        toolbar.frame = NSRect(x: 0, y: 0, width: b.width, height: tbH)
        let segW = filterSeg.intrinsicContentSize.width
        filterSeg.frame = NSRect(x: 12, y: (tbH - JiraTheme.height) / 2, width: segW, height: JiraTheme.height)
        var bx = b.width - 10
        for btn in toolButtons.reversed() {
            bx -= 28
            btn.frame = NSRect(x: bx, y: (tbH - 26) / 2, width: 26, height: 26)
            bx -= 2
        }
        let fw: CGFloat = findMode == .none ? 0 : 220
        findBox.frame = NSRect(x: bx - fw - 6, y: (tbH - JiraTheme.height) / 2, width: fw, height: JiraTheme.height)
        summary.frame = NSRect(x: filterSeg.frame.maxX + 14, y: (tbH - 18) / 2, width: max(40, findBox.frame.minX - filterSeg.frame.maxX - 24), height: 18)
        var y = tbH
        let hh: CGFloat = 26
        let thumbW: CGFloat = 14
        let paneLeft = thumbW
        let pw = pane.paneW
        headerL.frame = NSRect(x: paneLeft, y: y, width: pw, height: hh)
        headerR.frame = NSRect(x: paneLeft + pw + pane.gutterW, y: y, width: max(10, b.width - paneLeft - pw - pane.gutterW), height: hh)
        if let s = pathEditSide {
            let h = s == .left ? headerL : headerR
            pathEdit.frame = h.frame.insetBy(dx: 4, dy: 1)
        }
        y += hh
        if !banner.isHidden {
            banner.frame = NSRect(x: 0, y: y, width: b.width, height: 32)
            bannerText.frame = NSRect(x: 14, y: 8, width: b.width - 220, height: 18)
            bannerKeep.frame = NSRect(x: b.width - 100, y: 5, width: 88, height: 22)
            bannerReload.frame = NSRect(x: b.width - 190, y: 5, width: 84, height: 22)
            y += 32
        }
        let statusH: CGFloat = 24
        let detH: CGFloat = showDetails ? pane.rowH * 2 + 10 : 0
        let mainH = max(40, b.height - y - statusH - detH)
        thumb.frame = NSRect(x: 0, y: y, width: thumbW, height: mainH)
        scroll.frame = NSRect(x: thumbW, y: y, width: b.width - thumbW, height: mainH)
        emptyHint.frame = NSRect(x: thumbW, y: y + mainH / 2 - 10, width: b.width - thumbW, height: 20)
        details.isHidden = !showDetails
        details.frame = NSRect(x: 0, y: y + mainH, width: b.width, height: detH)
        status.frame = NSRect(x: 12, y: y + mainH + detH + 4, width: b.width - 24, height: 16)
        sizePane()
        // the header follows the pane's columns
        headerL.frame.size.width = pane.paneW
        headerR.frame.origin.x = paneLeft + pane.paneW + pane.gutterW
    }

    private func sizePane() {
        let w = scroll.contentSize.width
        let h = max(scroll.contentSize.height, pane.docHeight)
        if pane.frame.size != NSSize(width: w, height: h) { pane.frame = NSRect(x: 0, y: 0, width: w, height: h) }
        positionEditor()
    }

    // MARK: pages

    private func showPage() {
        persistSoon()
        let onStart = session == nil
        let isFolder = session?.folder != nil
        start.isHidden = !onStart
        body.isHidden = onStart || isFolder
        folderPage.root.isHidden = !isFolder
        pills.titles = sessions.map(\.label)
        pills.badges = sessions.map { $0.isDirty ? PopupTabBadge(tone: .warning, text: "", tip: "unsaved changes") : nil }
        pills.selected = selected ?? -1
        if pills.vertical { pills.pinnedSelected = onStart ? "home" : nil }
        root?.needsLayout = true
        if onStart { reloadRecent() }
    }

    func showStartPage() {
        commitEditor()
        selected = nil
        showPage()
        window.makeFirstResponder(leftBox.field)
    }

    private func select(_ i: Int) {
        guard sessions.indices.contains(i) else { return }
        commitEditor()
        session.map { if $0.folder == nil { $0.scrollY = scroll.contentView.bounds.origin.y } }
        selected = i
        closeFind()
        cancelPathEdit()
        showPage()
        bindSession()
    }

    // the selected session onto the panes
    private func bindSession() {
        guard let s = session else { return }
        if let f = s.folder {
            folderPage.bind(f)
            window.makeFirstResponder(folderPage.tree)
            return
        }
        pane.invalidateMarks()
        sizePane()
        layoutText(body.bounds)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(s.scrollY, max(0, pane.frame.height - scroll.contentSize.height))))
        scroll.reflectScrolledClipView(scroll.contentView)
        updateBanner()
        syncAll()
        window.makeFirstResponder(pane)
    }

    // everything that reads the session: pills, headers, summary, status, thumbnail
    private func syncAll() {
        persistSoon()
        guard let s = session else {
            showPage()
            return
        }
        pills.titles = sessions.map(\.label)
        pills.badges = sessions.map { $0.isDirty ? PopupTabBadge(tone: .warning, text: "", tip: "unsaved changes") : nil }
        pills.selected = selected ?? -1
        if s.folder != nil { return }       // the folder page paints itself (FolderPage.sync)
        filterSeg.selected = CompareFilter.allCases.firstIndex(of: s.filter) ?? 0
        for (h, side) in [(headerL, CompareSide.left), (headerR, .right)] {
            h.text = s.title[side].map { "\($0)  ·  \(s.path[side].map(tilde) ?? "")" } ?? s.path[side].map(tilde)
                ?? (s.model.side(side).lines.isEmpty ? "(empty — paste ⌘V, drop a file, or ⌘O)" : "(pasted text)")
            h.dirty = s.dirty(side)
            h.focused = s.focus == side
        }
        let m = s.model
        if s.isBinary {
            summary.stringValue = binarySummary(s)
        } else if m.sections.isEmpty {
            summary.stringValue = identicalSummary(s)
        } else {
            summary.stringValue = "≠ \(m.sections.count) section\(m.sections.count == 1 ? "" : "s")"
                + (m.unimportantCount > 0 ? "  (\(m.importantCount) important · \(m.unimportantCount) unimportant)" : "")
        }
        summary.textColor = m.sections.isEmpty ? colors.tone(.success) : colors.text
        emptyHint.isHidden = !(s.displayCount == 0 && !s.isBinary)
        emptyHint.stringValue = cfg.label("empty", "Both sides are empty — paste text (⌘V), drop files, or open them (⌘O)")
        scroll.isHidden = s.isBinary
        status.stringValue = statusLine(s)
        let lo = scroll.contentView.bounds.minY, h = scroll.contentView.bounds.height, total = max(1, pane.frame.height)
        thumb.visibleFraction = (lo / total, h / total)
        thumb.needsDisplay = true
        details.needsDisplay = true
        pane.needsDisplay = true
    }

    private func tilde(_ p: String) -> String { (p as NSString).abbreviatingWithTildeInPath }

    private func identicalSummary(_ s: CompareSession) -> String {
        let l = s.model.left, r = s.model.right
        if l.lines.isEmpty && r.lines.isEmpty { return "" }
        if let a = s.disk[.left], let b = s.disk[.right], a == b, !s.isDirty { return cfg.label("same", "Identical") }
        if l.encoded() == r.encoded() { return cfg.label("same", "Identical") }
        // the text reads the same: say what differs (never "identical")
        var why: [String] = []
        if l.encoding != r.encoding { why.append("encoding \(l.encoding.rawValue) vs \(r.encoding.rawValue)") }
        if l.eolLabel != r.eolLabel || l.eols.last != r.eols.last { why.append("line endings") }
        if s.model.ignoreUnimportant && s.model.rows.contains(where: { $0.kind != .same }) { why.append("unimportant differences") }
        if why.isEmpty { why.append("bytes (unimportant differences ignored)") }
        return cfg.label("same-text", "Same text — files differ in {}", why.joined(separator: ", "))
    }

    private func binarySummary(_ s: CompareSession) -> String {
        if s.tooLarge.values.contains(true) && s.binary.isEmpty {
            // too big to show: a byte compare only
            let a = s.disk[.left] ?? Data(), b = s.disk[.right] ?? Data()
            let d = BinaryCompare.firstDifference(a, b)
            return cfg.label("too-large", "Too large for Text Compare ([compare] max-lines): {}",
                             d == nil ? "identical bytes" : "differ (first difference at byte \(d!))")
        }
        let a = s.binary[.left] ?? s.disk[.left] ?? Data(), b = s.binary[.right] ?? s.disk[.right] ?? Data()
        guard let d = BinaryCompare.firstDifference(a, b) else { return cfg.label("binary", "Binary files {}", "are identical") }
        return cfg.label("binary", "Binary files {}",
                         "differ (\(ByteCountFormatter.string(fromByteCount: Int64(a.count), countStyle: .file)) vs "
                         + "\(ByteCountFormatter.string(fromByteCount: Int64(b.count), countStyle: .file)), first difference at byte \(d))")
    }

    private func statusLine(_ s: CompareSession) -> String {
        let m = s.model
        var parts: [String] = []
        if !s.isBinary {
            parts.append("\(m.importantCount) important · \(m.unimportantCount) unimportant")
            if s.displayCount > 0 {
                let row = m.rows[s.modelRow(s.cursor)]
                let line = row.line(s.focus)
                parts.append("\(s.focus == .left ? "L" : "R") line \(line >= 0 ? String(line + 1) : "—")")
            }
        }
        func side(_ x: CompareSide) -> String {
            let t = m.side(x)
            return "\(t.encoding.rawValue) · \(t.eolLabel)" + (s.dirty(x) ? " · edited" : "")
                + (s.changedOnDisk[x] == true ? " · changed on disk" : "")
        }
        parts.append(side(.left) + "   ‖   " + side(.right))
        if !s.status.isEmpty { parts.append(s.status) }
        return parts.joined(separator: "   ·   ")
    }

    // MARK: ComparePaneHost

    var paneSession: CompareSession? { session }
    var paneColors: PopupColors { colors }
    var paneGutterArrows: String { cfg.gutterArrows }
    var paneTabWidth: Int { cfg.tabWidth }
    var paneShowWhitespace: Bool { showWhitespace }
    var paneAlignPick: (side: CompareSide, line: Int)? { alignPick }

    func paneClicked(row: Int, side: CompareSide, col: Int, clicks: Int, shift: Bool) {
        guard let s = session else { return }
        commitEditor()
        if shift {
            if s.anchor == nil { s.anchor = s.cursor }
        } else {
            s.anchor = nil
        }
        s.cursor = row
        s.focus = side
        s.col = col
        if clicks >= 2 { beginEdit() }
        syncAll()
    }

    func paneDragged(to row: Int) {
        guard let s = session else { return }
        if s.anchor == nil { s.anchor = s.cursor }
        s.cursor = row
        syncAll()
    }

    func paneGutterCopy(section: Int, from: CompareSide) {
        guard let s = session else { return }
        commitEditor()
        s.cursor = s.displayRow(s.model.sections[section].rows.lowerBound)
        copyAcross(from: from)
    }

    func paneScrolled() {
        session?.scrollY = scroll.contentView.bounds.origin.y
        let lo = scroll.contentView.bounds.minY, h = scroll.contentView.bounds.height, total = max(1, pane.frame.height)
        thumb.visibleFraction = (lo / total, h / total)
    }

    private func scrollToFraction(_ f: CGFloat) {
        let y = f * pane.frame.height - scroll.contentSize.height / 2
        scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, min(y, pane.frame.height - scroll.contentSize.height))))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    func paneDrop(side: CompareSide, urls: [URL], text: String?) -> Bool {
        if let u = urls.first {
            if session == nil { _ = openPair(side == .left ? u.path : nil, side == .right ? u.path : nil) }
            else { openFile(u.path, into: side) }
            return true
        }
        if let t = text, !t.isEmpty { setPasted(t, side); return true }
        return false
    }

    // MARK: sessions

    // open a pair (either side may be nil = empty, takes a paste / drop).
    // Folders are phase 2: refused with a word on the start page.
    @discardableResult
    func openPair(_ left: String?, _ right: String?, titles: [CompareSide: String] = [:], git: Bool = false,
                  waiter: (() -> Void)? = nil, then: ((CompareSession) -> Void)? = nil) -> CompareSession? {
        let t0 = DispatchTime.now().uptimeNanoseconds
        cfg = CompareConfig.load()
        let fm = FileManager.default
        func isDir(_ p: String?) -> Bool? {
            guard let p else { return nil }
            var d: ObjCBool = false
            return fm.fileExists(atPath: p, isDirectory: &d) ? d.boolValue : false
        }
        let dirs = [left, right].compactMap(isDir)
        if dirs.contains(true) {
            // folders: both sides must be folders
            if let l = left, let r = right, isDir(l) == true, isDir(r) == true {
                return openFolders(l, r, titles: titles, git: git, waiter: waiter)
            }
            showStartPage()
            let one = [left, right].compactMap { $0 }.count == 1
            showStartHint(cfg.label("folder", "Folder Compare needs a folder on both sides: {}",
                                              one ? "only one was given" : "a folder can't be compared with a file"), .warning)
            waiter?()
            return nil
        }
        let s = CompareSession(model: TextCompare())
        s.git = git
        s.title = titles
        var loaded: [CompareSide: TextSide] = [:]
        for (side, p) in [(CompareSide.left, left), (.right, right)] {
            guard let p else { continue }
            s.path[side] = p
            loaded[side] = load(p, into: s, side: side)
        }
        commitEditor()
        sessions.append(s)
        selected = sessions.count - 1
        if let waiter { s.waiters.append(waiter) }
        let lines = (loaded[.left]?.lines.count ?? 0) + (loaded[.right]?.lines.count ?? 0)
        let imp = cfg.importance
        let finish: (TextCompare, Double) -> Void = { [weak self] model, diffMs in
            guard let self else { return }
            s.model = model
            s.refresh(context: self.cfg.contextLines)
            if let first = model.sections.first { s.cursor = s.displayRow(first.rows.lowerBound) }
            then?(s)
            self.showPage()
            self.bindSession()
            self.scrollCursorVisible(center: true)
            let tp = DispatchTime.now().uptimeNanoseconds
            self.window.contentView?.layoutSubtreeIfNeeded()
            self.window.displayIfNeeded()
            let paint = Double(DispatchTime.now().uptimeNanoseconds - tp) / 1_000_000
            let total = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
            self.controller?.log(String(format: "compare text: %d lines, diff %.0f ms, paint %.0f ms (open → painted %.0f ms)",
                                        lines, diffMs, paint, total))
        }
        if let l = left, let r = right, !git, !s.isBinary, !restoring { CompareRecent.add(l, r, limit: cfg.recentLimit) }
        startHint.stringValue = ""
        let l = loaded[.left] ?? TextSide(), r = loaded[.right] ?? TextSide()
        if lines > 60_000 {
            // big: diff off the main thread (Esc stops it), the view shows at once
            diffGen += 1
            let gen = diffGen
            diffRunning = true
            s.status = "comparing…"
            showPage()
            bindSession()
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let td = DispatchTime.now().uptimeNanoseconds
                let model = TextCompare(left: l, right: r, importance: imp)
                let ms = Double(DispatchTime.now().uptimeNanoseconds - td) / 1_000_000
                DispatchQueue.main.async {
                    guard let self, gen == self.diffGen else { return }
                    self.diffRunning = false
                    s.status = ""
                    finish(model, ms)
                }
            }
        } else {
            let td = DispatchTime.now().uptimeNanoseconds
            let model = TextCompare(left: l, right: r, importance: imp)
            finish(model, Double(DispatchTime.now().uptimeNanoseconds - td) / 1_000_000)
        }
        for side in [CompareSide.left, .right] where s.path[side] != nil { watch(s, side) }
        return s
    }

    // two folders: a Folder Compare session (the scan streams in on its own)
    @discardableResult
    func openFolders(_ left: String, _ right: String, titles: [CompareSide: String] = [:], git: Bool = false,
                     waiter: (() -> Void)? = nil) -> CompareSession? {
        cfg = CompareConfig.load()
        commitEditor()
        let s = CompareSession(model: TextCompare())
        s.git = git
        s.title = titles
        s.path = [.left: left, .right: right]
        s.folder = FolderSession(left: left, right: right)
        s.folder?.alwaysContent = git
        sessions.append(s)
        selected = sessions.count - 1
        if let waiter { s.waiters.append(waiter) }
        if !git && !restoring { CompareRecent.add(left, right, limit: cfg.recentLimit) }
        startHint.stringValue = ""
        closeFind()
        cancelPathEdit()
        showPage()
        bindSession()
        return s
    }

    // MARK: session restore + recovery copies (PRD §7.3.4)
    //
    // compare-sessions.json = the open sessions (pairs, filters, cursor,
    // importance, alignments); compare-recovery/ = the text of every side
    // with unsaved edits (and pasted sides), written 2 s after the last
    // change, on hide and on quit. A restart brings them back as
    // "unsaved (recovered)"; a save / discard drops the copy. git sessions
    // (temp files) are never kept.

    static var stateDir: String { NSHomeDirectory() + "/.cache/kitchen-sink" }
    static var sessionsPath: String { stateDir + "/compare-sessions.json" }
    static var recoveryDir: String { stateDir + "/compare-recovery" }
    private var persistWork: DispatchWorkItem?
    private var restoring = false

    private func persistSoon() {
        guard !isSub, !restoring else { return }
        persistWork?.cancel()
        let w = DispatchWorkItem { [weak self] in self?.persistNow() }
        persistWork = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: w)
    }

    func persistNow() {
        guard !isSub, !restoring else { return }
        persistWork?.cancel()
        persistWork = nil
        let fm = FileManager.default
        try? fm.createDirectory(atPath: Self.recoveryDir, withIntermediateDirectories: true)
        var keep = Set<String>()
        var out: [[String: Any]] = []
        var sel: Int?
        for s in sessions where !s.git {
            if s === session { sel = out.count }
            var o: [String: Any] = [:]
            if let f = s.folder {
                o["kind"] = "folder"
                o["left"] = f.leftRoot
                o["right"] = f.rightRoot
                o["filter"] = f.view.filter.rawValue
                o["flatten"] = f.view.flatten
                o["names"] = f.view.nameFilter
                o["hidden"] = f.hidden
                o["exclude"] = f.extraExclude
                if f.rows.indices.contains(f.cursor) { o["cursorKey"] = f.rows[f.cursor].node.key }
                out.append(o)
                continue
            }
            o["kind"] = "text"
            o["filter"] = s.filter.rawValue
            o["cursor"] = s.cursor
            o["focus"] = s.focus.rawValue
            let i = s.model.importance
            o["importance"] = [i.leadingWS, i.trailingWS, i.embeddedWS, i.ignoreCase, i.lineEndings, i.blankLines, s.model.ignoreUnimportant]
            o["anchors"] = s.model.anchors.map { [$0.l, $0.r] }
            for side in [CompareSide.left, .right] {
                let k = side.rawValue
                if let p = s.path[side] { o[k] = p }
                if let t = s.title[side] { o[k + "Title"] = t }
                let pasted = s.path[side] == nil && !s.model.side(side).lines.isEmpty
                guard s.dirty(side) || pasted, s.binary[side] == nil, s.tooLarge[side] == nil else { continue }
                let file = Self.recoveryDir + "/\(s.id)-\(k).txt"
                if s.recoveryVersion[side] != s.version || !fm.fileExists(atPath: file) {
                    let t = s.model.side(side)
                    let data = t.encoded() ?? Data(t.text.utf8)
                    try? data.write(to: URL(fileURLWithPath: file), options: .atomic)
                    s.recoveryVersion[side] = s.version
                }
                keep.insert(file)
                o[k + "Recovery"] = file
                o[k + "Pasted"] = pasted
            }
            out.append(o)
        }
        // a copy no session needs any more (saved, discarded, closed) goes
        for f in (try? fm.contentsOfDirectory(atPath: Self.recoveryDir)) ?? [] where !keep.contains(Self.recoveryDir + "/" + f) {
            try? fm.removeItem(atPath: Self.recoveryDir + "/" + f)
        }
        let obj: [String: Any] = ["sessions": out, "selected": sel ?? -1]
        if let d = try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted]) {
            try? d.write(to: URL(fileURLWithPath: Self.sessionsPath), options: .atomic)
        }
    }

    private func restoreSessions() {
        guard let d = FileManager.default.contents(atPath: Self.sessionsPath),
              let obj = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any],
              let arr = obj["sessions"] as? [[String: Any]], !arr.isEmpty else { return }
        restoring = true
        let fm = FileManager.default
        func isDir(_ p: String) -> Bool { var b: ObjCBool = false; return fm.fileExists(atPath: p, isDirectory: &b) && b.boolValue }
        var recoveredSides = 0
        for e in arr {
            if e["kind"] as? String == "folder" {
                guard let l = e["left"] as? String, let r = e["right"] as? String, isDir(l), isDir(r),
                      let s = openFolders(l, r), let f = s.folder else { continue }
                f.view.filter = FolderFilter(rawValue: e["filter"] as? String ?? "") ?? .all
                f.view.flatten = e["flatten"] as? Bool ?? false
                f.view.nameFilter = e["names"] as? String ?? ""
                f.hidden = e["hidden"] as? Bool ?? true
                f.extraExclude = e["exclude"] as? [String] ?? []
                f.cursorKey = e["cursorKey"] as? String
                continue
            }
            var paths: [CompareSide: String] = [:], recovery: [CompareSide: (file: String, pasted: Bool)] = [:]
            var titles: [CompareSide: String] = [:]
            for side in [CompareSide.left, .right] {
                let k = side.rawValue
                if let p = e[k] as? String, fm.fileExists(atPath: p), !isDir(p) { paths[side] = p }
                if let t = e[k + "Title"] as? String { titles[side] = t }
                if let f = e[k + "Recovery"] as? String, fm.fileExists(atPath: f) {
                    let pasted = e[k + "Pasted"] as? Bool ?? false
                    // a file that is gone keeps its unsaved text as a pasted side
                    recovery[side] = (f, pasted || paths[side] == nil)
                }
            }
            guard !paths.isEmpty || !recovery.isEmpty else { continue }
            openPair(paths[.left], paths[.right], titles: titles) { s in
                if let b = e["importance"] as? [Bool], b.count == 7 {
                    let imp = Importance(leadingWS: b[0], trailingWS: b[1], embeddedWS: b[2], ignoreCase: b[3], lineEndings: b[4], blankLines: b[5])
                    if imp != s.model.importance { s.model.setImportance(imp) }
                    if b[6] != s.model.ignoreUnimportant { s.model.setIgnoreUnimportant(b[6]) }
                }
                for (side, r) in recovery {
                    guard let data = fm.contents(atPath: r.file) else { continue }
                    let t = TextSide.decode(data) ?? TextSide(text: String(decoding: data, as: UTF8.self))
                    if r.pasted {
                        s.path[side] = nil
                        s.model.setSide(side, t)
                        s.markClean(side)
                        continue
                    }
                    let cur = s.model.side(side)
                    guard cur.lines != t.lines || cur.eols != t.eols else { continue }    // saved after all
                    s.willEdit(side)
                    s.model.replace(side, 0..<cur.lines.count, lines: t.lines, eols: t.eols)
                    s.recovered = true
                    recoveredSides += 1
                }
                for a in (e["anchors"] as? [[Int]]) ?? [] where a.count == 2 { s.model.align(left: a[0], right: a[1]) }
                s.filter = CompareFilter(rawValue: e["filter"] as? String ?? "") ?? .all
                s.focus = CompareSide(rawValue: e["focus"] as? String ?? "") ?? .left
                s.refresh(context: self.cfg.contextLines)
                s.cursor = min(max(0, e["cursor"] as? Int ?? 0), max(0, s.displayCount - 1))
                if s.recovered { s.status = self.cfg.label("recovered", "unsaved changes recovered after a restart — ⌘S saves, ⌘Z undoes") }
            }
        }
        restoring = false
        let sel = obj["selected"] as? Int ?? -1
        if sessions.indices.contains(sel) { select(sel) } else if !sessions.isEmpty { select(sessions.count - 1) }
        controller?.log("compare: restored \(sessions.count) session(s), \(recoveredSides) side(s) with recovered edits")
        persistSoon()
    }

    // MARK: FolderHost

    var folderWindow: NSWindow { window }
    var folderColors: PopupColors { colors }
    func folderPrompt(_ title: String, info: String, text: String, ok: String, then: @escaping (String?) -> Void) {
        prompt(title, info: info, text: text, ok: ok, colors: colors, then: then)
    }
    func folderConfirm(_ title: String, info: String, accessory: NSView?, choices: [ConfirmOverlay.Choice],
                       defaultIndex: Int, cancelIndex: Int, then: @escaping (Int) -> Void) {
        confirm(title, info: info, accessory: accessory, choices: choices,
                defaultIndex: defaultIndex, cancelIndex: cancelIndex, colors: colors, then: then)
    }
    var folderConfig: CompareConfig { cfg }
    func folderChanged() {
        pills?.titles = sessions.map(\.label)
        persistSoon()
    }
    func folderLog(_ s: String) { controller?.log(s) }
    func folderOpenNote(_ path: String) { controller?.openNoteFile(path) }

    // Return / double-click on a file pair: a Text Compare ON the view (Esc = back,
    // same row). Without the shared window it opens as another session here.
    func folderOpenPair(_ left: String?, _ right: String?) {
        guard let c = controller, !isSub, settings.sharedWindow else { _ = openPair(left, right); return }
        let sub = CompareWindow.createSub(controller: c, frame: c.slot.currentFrame())
        sub.openSubPair(left, right)
        c.slot.push(.compareText)
    }

    // the pushed Text Compare: the same pair again keeps its edits + scroll;
    // another pair replaces the clean sessions
    func openSubPair(_ left: String?, _ right: String?) {
        if let s = sessions.first(where: { $0.path[.left] == left && $0.path[.right] == right && $0.folder == nil }),
           let i = sessions.firstIndex(where: { $0 === s }) {
            select(i)
            return
        }
        for i in sessions.indices.reversed() where !sessions[i].isDirty {
            let old = sessions.remove(at: i)
            old.stopWatching()
            let w = old.waiters
            old.waiters = []
            w.forEach { $0() }
        }
        selected = nil
        _ = openPair(left, right)
    }

    func folderSwapped(old: FolderSession, new: FolderSession) {
        guard let i = sessions.firstIndex(where: { $0.folder === old }) else { return }
        let s = sessions[i]
        old.gen.bump()
        s.folder = new
        s.path = [.left: new.leftRoot, .right: new.rightRoot]
        if !s.git { CompareRecent.add(new.leftRoot, new.rightRoot, limit: cfg.recentLimit) }
        showPage()
        bindSession()
    }

    // read a side's file: text, binary (byte compare) or too large
    private func load(_ p: String, into s: CompareSession, side: CompareSide) -> TextSide? {
        guard let d = FileManager.default.contents(atPath: p) else {
            s.status = "can't read \(tilde(p))"
            return nil
        }
        s.disk[side] = d
        s.binary[side] = nil
        s.tooLarge[side] = nil
        if d.count > 50 * 1024 * 1024 { s.tooLarge[side] = true; return nil }
        if TextSide.isBinary(d) { s.binary[side] = d; return nil }
        guard let t = TextSide.decode(d) else { s.binary[side] = d; return nil }
        if t.lines.count > cfg.maxLines { s.tooLarge[side] = true; return nil }
        return t
    }

    // a file into one side of the current session (Cmd+O, a drop, the path field)
    func openFile(_ p: String, into side: CompareSide) {
        guard let s = session else { _ = openPair(side == .left ? p : nil, side == .right ? p : nil); return }
        commitEditor()
        if s.folder == nil, s.dirty(side) {
            askDiscard(s, side, what: cfg.label("replace-info", "Opening {} replaces them.", "“\((p as NSString).lastPathComponent)”")) { [weak self] in
                self?.placeFile(p, into: side)
            }
            return
        }
        placeFile(p, into: side)
    }

    // unsaved edits on one side stand in the way of a replacement: Save is the
    // default (Return), Discard is red, Esc = Cancel
    private func askDiscard(_ s: CompareSession, _ side: CompareSide, what: String, then go: @escaping () -> Void) {
        let name = (s.path[side] as NSString?)?.lastPathComponent ?? side.rawValue
        confirm(cfg.label("unsaved-side", "Unsaved changes in {}", "“\(name)”"),
                info: what,
                choices: [(cfg.label("discard-button", "Discard"), .danger),
                          (cfg.label("cancel-button", "Cancel"), .normal),
                          (cfg.label("save-button", "Save"), .primary)],
                defaultIndex: 2, cancelIndex: 1, colors: colors) { [weak self] i in
            guard let self, self.session === s else { return }
            switch i {
            case 2: self.save(side) { ok in if ok { go() } }
            case 0: go()
            default: break
            }
        }
    }

    private func placeFile(_ p: String, into side: CompareSide) {
        guard let s = session else { return }
        var dir: ObjCBool = false
        if FileManager.default.fileExists(atPath: p, isDirectory: &dir), dir.boolValue {
            s.status = cfg.label("folder", "Folder Compare needs a folder on both sides: {}", "\(tilde(p)) is a folder")
            syncAll()
            return
        }
        commitEditor()
        s.watchers[side]?.cancel()
        s.watchers[side] = nil
        s.path[side] = p
        s.title[side] = nil
        let t = load(p, into: s, side: side) ?? TextSide()
        s.model.setSide(side, t)
        s.markClean(side)
        s.changedOnDisk[side] = nil
        modelChanged(s)
        watch(s, side)
        if let l = s.path[.left], let r = s.path[.right], !s.git { CompareRecent.add(l, r, limit: cfg.recentLimit) }
    }

    // pasted text as one side (a pane's Cmd+V, "Paste Clipboard Here"). An empty
    // side takes it as its content; a side with text is REPLACED by an
    // undoable edit (Cmd+Z brings the old text back)
    func setPasted(_ text: String, _ side: CompareSide) {
        if session == nil || session?.folder != nil { openPair(nil, nil) }
        guard let s = session else { return }
        commitEditor()
        let had = s.model.side(side).lines.count
        if had > 0 && s.binary[side] == nil && s.tooLarge[side] == nil {
            let lines = TextSide(text: text).lines
            s.willEdit(side)
            s.model.replace(side, 0..<had, with: lines)
            s.status = "replaced the \(side.rawValue) side (⌘Z undoes)"
            modelChanged(s)
            return
        }
        s.watchers[side]?.cancel()
        s.watchers[side] = nil
        s.path[side] = nil
        s.disk[side] = nil
        s.binary[side] = nil
        s.tooLarge[side] = nil
        s.model.setSide(side, TextSide(text: text))
        s.markClean(side)
        s.status = "pasted into the \(side.rawValue) side"
        modelChanged(s)
    }

    // the model changed wholesale: filter, caches, cursor, views
    private func modelChanged(_ s: CompareSession, keepCursor: Bool = false) {
        s.refresh(context: cfg.contextLines)
        if !keepCursor, let first = s.model.sections.first { s.cursor = s.displayRow(first.rows.lowerBound) }
        pane.invalidateMarks()
        sizePane()
        syncAll()
    }

    // a pair with pasted text isn't lost with its pill: both sides are written
    // as snapshot files (file-backed sides keep their path) → one Recent row
    private func rememberPasted(_ s: CompareSession) {
        guard s.folder == nil, !s.git, !s.isBinary, cfg.recentLimit > 0 else { return }
        let sides: [CompareSide] = [.left, .right]
        guard sides.contains(where: { s.path[$0] == nil && !s.model.side($0).lines.isEmpty }),
              sides.allSatisfy({ s.path[$0] != nil || !s.model.side($0).lines.isEmpty }) else { return }
        var out: [CompareSide: String] = [:]
        for side in sides {
            if let p = s.path[side] { out[side] = p; continue }
            guard let d = s.model.side(side).encoded() ?? Optional(Data(s.model.side(side).lines.joined(separator: "\n").utf8)) else { return }
            let p = CompareRecent.pastedDir + "/\(Int(Date().timeIntervalSince1970))-\(s.id)-\(side.rawValue).txt"
            try? FileManager.default.createDirectory(atPath: CompareRecent.pastedDir, withIntermediateDirectories: true)
            guard (try? d.write(to: URL(fileURLWithPath: p))) != nil else { return }
            out[side] = p
        }
        if let l = out[.left], let r = out[.right] { CompareRecent.add(l, r, limit: cfg.recentLimit) }
    }

    private func removeSession(_ i: Int) {
        guard sessions.indices.contains(i) else { return }
        let s = sessions.remove(at: i)
        s.folder?.gen.bump()
        rememberPasted(s)
        s.stopWatching()
        let w = s.waiters
        s.waiters = []
        w.forEach { $0() }
        if sessions.isEmpty {
            selected = nil
        } else if let sel = selected {
            selected = sel > i ? sel - 1 : min(sel, sessions.count - 1)
        }
        if isSub && sessions.isEmpty, let c = controller {
            // the pushed text compare closed: back to the view under it
            c.slot.back()
        }
        showPage()
        if session != nil { bindSession() }
    }

    // the pill's ✕ / Close: unsaved edits ask first (a sheet, never app-modal)
    func closeSession(_ i: Int, force: Bool = false) {
        guard sessions.indices.contains(i) else { return }
        commitEditor()
        let s = sessions[i]
        guard s.isDirty, !force else { removeSession(i); return }
        if selected != i { select(i) }
        // Save is the default (Return), Don't Save is red, Esc = Cancel
        confirm(cfg.label("unsaved", "Save changes to {}?", s.label),
                info: cfg.label("unsaved-info", "Your edits are lost if you don't save them."),
                choices: [(cfg.label("discard-button", "Don't Save"), .danger),
                          (cfg.label("cancel-button", "Cancel"), .normal),
                          (cfg.label("save-button", "Save"), .primary)],
                defaultIndex: 2, cancelIndex: 1, colors: colors) { [weak self] i in
            guard let self, let idx = self.sessions.firstIndex(where: { $0 === s }) else { return }
            switch i {
            case 2: self.saveAll(s) { ok in if ok { self.removeSession(idx) } }
            case 0: self.removeSession(idx)
            default: break
            }
        }
    }

    // MARK: start page

    private func compareFromStart() {
        let l = expand(leftBox.field.stringValue), r = expand(rightBox.field.stringValue)
        if l.isEmpty && r.isEmpty {
            if recentList.rows.indices.contains(recentList.selection) { openRecent(recentList.selection); return }
            openPair(nil, nil)
            return
        }
        for p in [l, r] where !p.isEmpty && !FileManager.default.fileExists(atPath: p) {
            showStartHint("No such file: \(tilde(p))", .danger)
            return
        }
        openPair(l.isEmpty ? nil : l, r.isEmpty ? nil : r)
    }

    // "New Text Compare": two EMPTY sides. Nothing is read from the
    // clipboard — click a side and ⌘V (or type); the diff runs as soon as
    // both sides hold text
    func startPasted() {
        guard let s = openPair(nil, nil) else { return }
        s.focus = .left
        s.status = cfg.label("paste-next", "click a side and paste (⌘V) or type — left first, then right")
        syncAll()
        window.makeFirstResponder(pane)
    }

    private func expand(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? "" : (t as NSString).expandingTildeInPath
    }

    // the message line under the start page's buttons (sized by layoutStart,
    // so the START page must lay out again — root alone left it 0 pt tall)
    private func showStartHint(_ text: String, _ tone: PopupTone) {
        startHint.stringValue = text
        startHint.textColor = colors.tone(tone)
        root.needsLayout = true
        start.needsLayout = true
    }

    // "READY": what the user was just doing, one click each: a path marked
    // in the file browser.
    private func refreshSuggestions() {
        var items: [CapsuleButtons.Item] = []
        let fm = FileManager.default
        func fill(_ path: String) {
            let t = tilde(path)
            if leftBox.field.stringValue.isEmpty { leftBox.field.stringValue = t }
            else if rightBox.field.stringValue.isEmpty { rightBox.field.stringValue = t }
            else { rightBox.field.stringValue = t }
            window.makeFirstResponder(leftBox.field.stringValue.isEmpty || rightBox.field.stringValue.isEmpty ? leftBox.field : rightBox.field)
            if !leftBox.field.stringValue.isEmpty && !rightBox.field.stringValue.isEmpty { compareFromStart() }
            refreshSuggestions()
        }
        if let pick = FileListPane.comparePick, fm.fileExists(atPath: pick),
           expand(leftBox.field.stringValue) != pick, expand(rightBox.field.stringValue) != pick {
            items.append(.init(title: "Marked: " + (pick as NSString).lastPathComponent, symbol: "checkmark.circle", primary: false) { fill(pick) })
        }
        // (the clipboard is never guessed at: you paste into the side you pick)
        suggestActions?.items = items
        start.needsLayout = true
    }

    private func reloadRecent() {
        refreshSuggestions()
        recentAll = CompareRecent.load()
        filterRecent()
        let fm = FileManager.default
        let missing = recentAll.filter { !fm.fileExists(atPath: $0.left) || !fm.fileExists(atPath: $0.right) }.count
        var items: [CapsuleButtons.Item] = []
        if missing > 0 {
            items.append(.init(title: "Clear Missing (\(missing))", symbol: "exclamationmark.triangle", primary: false) { [weak self] in
                self?.clearMissingRecents()
            })
        }
        if !recentAll.isEmpty {
            items.append(.init(title: "Clear All", symbol: "trash", primary: false) { [weak self] in self?.clearAllRecents() })
        }
        recentActions?.items = items
        start.needsLayout = true
    }

    private func filterRecent() {
        let q = recentFilter.field.stringValue.lowercased()
        recentList.rows = q.isEmpty ? recentAll : recentAll.filter { ($0.left + " " + $0.right).lowercased().contains(q) }
        root?.needsLayout = true
        start.needsLayout = true
    }

    private func openRecent(_ i: Int) {
        guard recentList.rows.indices.contains(i) else { return }
        let e = recentList.rows[i]
        recentList.refreshMissing()
        let missing = recentList.missingPaths(i)
        if !missing.isEmpty {
            let names = missing.map { "“\(($0 as NSString).lastPathComponent)”" }.joined(separator: " and ")
            showStartHint("Can't open: \(names) \(missing.count == 1 ? "no longer exists" : "no longer exist") "
                          + "(hover the row for the full path). Click the row's ✕ or Clear Missing to drop it.", .danger)
            return
        }
        openPair(e.left, e.right)
    }

    private func clearMissingRecents() {
        let fm = FileManager.default
        CompareRecent.save(CompareRecent.load().filter { fm.fileExists(atPath: $0.left) && fm.fileExists(atPath: $0.right) })
        showStartHint("", .dim)
        reloadRecent()
    }
    private func clearAllRecents() {
        CompareRecent.clearAll()
        showStartHint("", .dim)
        reloadRecent()
    }

    private func recentMenu(_ i: Int) -> NSMenu {
        let m = NSMenu()
        let e = recentList.rows[i]
        m.addItem(menuItem("Open") { [weak self] in self?.openRecent(i) })
        m.addItem(menuItem("Fill the Path Fields") { [weak self] in
            self?.leftBox.field.stringValue = self?.tilde(e.left) ?? e.left
            self?.rightBox.field.stringValue = self?.tilde(e.right) ?? e.right
        })
        m.addItem(menuItem("Copy Paths") { copyText("\(e.left)\n\(e.right)") })
        m.addItem(.separator())
        m.addItem(menuItem("Remove from Recent") { [weak self] in CompareRecent.remove(e); self?.reloadRecent() })
        let fm = FileManager.default
        let gone = CompareRecent.load().filter { !fm.fileExists(atPath: $0.left) || !fm.fileExists(atPath: $0.right) }
        if !gone.isEmpty {
            m.addItem(menuItem("Remove All Missing (\(gone.count))") { [weak self] in
                CompareRecent.save(CompareRecent.load().filter { fm.fileExists(atPath: $0.left) && fm.fileExists(atPath: $0.right) })
                self?.showStartHint("", .dim)
                self?.reloadRecent()
            })
        }
        m.addItem(menuItem("Clear All Recents") { [weak self] in CompareRecent.clearAll(); self?.recentAll = []; self?.recentList.rows = [] })
        return m
    }

    private func browse(into box: JiraInputBox?) {
        guard let box else { return }
        let p = NSOpenPanel()
        p.canChooseFiles = true
        p.canChooseDirectories = true
        p.allowsMultipleSelection = false
        p.beginSheetModal(for: window) { r in
            guard r == .OK, let u = p.url else { return }
            box.field.stringValue = (u.path as NSString).abbreviatingWithTildeInPath
        }
    }

    // Tab in a path field: complete the name (the common prefix; a folder gets its "/")
    static func complete(_ typed: String) -> String {
        let exp = (typed as NSString).expandingTildeInPath
        let dir = exp.hasSuffix("/") ? exp : (exp as NSString).deletingLastPathComponent
        let pre = exp.hasSuffix("/") ? "" : (exp as NSString).lastPathComponent
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir.isEmpty ? "/" : dir) else { return typed }
        let hits = names.filter { $0.hasPrefix(pre) && (pre.hasPrefix(".") || !$0.hasPrefix(".")) }.sorted()
        guard let first = hits.first else { return typed }
        var common = first
        for h in hits.dropFirst() { while !h.hasPrefix(common) { common.removeLast() } }
        var out = (dir as NSString).appendingPathComponent(common)
        var isDir: ObjCBool = false
        if hits.count == 1, FileManager.default.fileExists(atPath: out, isDirectory: &isDir), isDir.boolValue { out += "/" }
        return typed.hasPrefix("~") ? (out as NSString).abbreviatingWithTildeInPath : out
    }

    // MARK: text field delegate (start page, find, path edit)

    func controlTextDidChange(_ n: Notification) {
        guard let f = n.object as? NSTextField else { return }
        if f === recentFilter.field { filterRecent() }
        if f === findBox.field, findMode == .find { find(f.stringValue, dir: 0) }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        let f = control as? NSTextField
        switch sel {
        case #selector(NSResponder.insertNewline(_:)):
            if f === findBox.field {
                if findMode == .goTo { goToLine(findBox.field.stringValue); closeFind() }
                else { find(findBox.field.stringValue, dir: NSEvent.modifierFlags.contains(.shift) ? -1 : 1) }
            } else if f === pathEdit.field {
                commitPathEdit()
            } else if f === leftBox.field && rightBox.field.stringValue.isEmpty && leftBox.field.stringValue.isEmpty {
                compareFromStart()
            } else if f === leftBox.field || f === rightBox.field || f === recentFilter.field {
                compareFromStart()
            }
            return true
        case #selector(NSResponder.insertTab(_:)):
            if let f, f === leftBox.field || f === rightBox.field || f === pathEdit.field, !f.stringValue.isEmpty {
                let c = Self.complete(f.stringValue)
                if c != f.stringValue {
                    f.stringValue = c
                    textView.string = c
                    textView.setSelectedRange(NSRange(location: (c as NSString).length, length: 0))
                    return true
                }
            }
            if f === leftBox.field { window.makeFirstResponder(rightBox.field); return true }
            if f === rightBox.field { window.makeFirstResponder(recentFilter.field); return true }
            if f === recentFilter.field { window.makeFirstResponder(leftBox.field); return true }
            return false
        case #selector(NSResponder.moveDown(_:)) where f === recentFilter.field || f === leftBox.field || f === rightBox.field:
            moveRecent(1)
            return true
        case #selector(NSResponder.moveUp(_:)) where f === recentFilter.field || f === leftBox.field || f === rightBox.field:
            moveRecent(-1)
            return true
        default:
            return false
        }
    }

    private func moveRecent(_ d: Int) {
        guard !recentList.rows.isEmpty else { return }
        recentList.selection = max(0, min(recentList.rows.count - 1, recentList.selection + d))
        recentList.scrollToVisible(NSRect(x: 0, y: CGFloat(recentList.selection) * recentList.rowH,
                                          width: 10, height: recentList.rowH))
    }

    // MARK: path edit (Cmd+L)

    private func beginPathEdit(_ side: CompareSide) {
        guard let s = session else { return }
        commitEditor()
        pathEditSide = side
        s.focus = side
        pathEdit.field.stringValue = s.path[side].map(tilde) ?? ""
        pathEdit.isHidden = false
        layoutText(body.bounds)
        window.makeFirstResponder(pathEdit.field)
        pathEdit.field.currentEditor()?.selectAll(nil)
        syncAll()
    }

    private func commitPathEdit() {
        guard let side = pathEditSide else { return }
        let p = expand(pathEdit.field.stringValue)
        cancelPathEdit()
        guard !p.isEmpty else { return }
        guard FileManager.default.fileExists(atPath: p) else {
            session?.status = "No such file: \(tilde(p))"
            syncAll()
            return
        }
        openFile(p, into: side)
    }

    private func cancelPathEdit() {
        guard pathEditSide != nil else { return }
        pathEditSide = nil
        pathEdit.isHidden = true
        window.makeFirstResponder(pane)
    }

    // MARK: find / go to line

    private func openFind(_ mode: FindMode) {
        guard session != nil else { return }
        commitEditor()
        findMode = mode
        findBox.isHidden = false
        findBox.field.placeholderString = mode == .goTo ? "go to line (\(session?.focus == .left ? "left" : "right"))" : "find — ⏎ next, ⇧⏎ previous"
        findBox.colors = colors
        if mode == .goTo { findBox.field.stringValue = "" }
        layoutText(body.bounds)
        window.makeFirstResponder(findBox.field)
        findBox.field.currentEditor()?.selectAll(nil)
    }

    private func closeFind() {
        guard findMode != .none else { return }
        findMode = .none
        findBox.isHidden = true
        layoutText(body.bounds)
        window.makeFirstResponder(pane)
    }

    // the next (dir 1) / previous (-1) line holding `q` on the focused side;
    // dir 0 = from the cursor (typing)
    private func find(_ q: String, dir: Int) {
        guard let s = session, !q.isEmpty, s.displayCount > 0 else { return }
        let n = s.displayCount
        let step = dir < 0 ? -1 : 1
        var d = dir == 0 ? s.cursor : s.cursor + step
        for _ in 0..<n {
            d = (d % n + n) % n
            let line = s.model.rows[s.modelRow(d)].line(s.focus)
            if line >= 0, s.model.side(s.focus).lines[line].range(of: q, options: .caseInsensitive) != nil {
                s.cursor = d
                s.anchor = nil
                s.status = ""
                scrollCursorVisible(center: true)
                syncAll()
                return
            }
            d += step
        }
        s.status = "“\(q)” not found on the \(s.focus.rawValue) side"
        syncAll()
    }

    private func goToLine(_ text: String) {
        guard let s = session, let n = Int(text.trimmingCharacters(in: .whitespaces)), n > 0 else { return }
        let target = n - 1
        if let m = s.model.rows.firstIndex(where: { $0.line(s.focus) >= target }) {
            if s.filter != .all { setFilter(.all) }
            s.cursor = s.displayRow(m)
            scrollCursorVisible(center: true)
            syncAll()
        }
    }

    // MARK: cursor + sections

    private func scrollCursorVisible(center: Bool = false) {
        guard let s = session else { return }
        let y = CGFloat(s.cursor) * pane.rowH
        let vis = scroll.contentView.bounds
        if center || y < vis.minY || y + pane.rowH > vis.maxY {
            let target = center ? y - vis.height / 3 : (y < vis.minY ? y : y + pane.rowH - vis.height)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, min(target, pane.frame.height - vis.height))))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
    }

    private func moveCursor(_ to: Int, extend: Bool = false) {
        guard let s = session, s.displayCount > 0 else { return }
        commitEditor()
        if extend { if s.anchor == nil { s.anchor = s.cursor } } else { s.anchor = nil }
        s.cursor = max(0, min(s.displayCount - 1, to))
        scrollCursorVisible()
        syncAll()
    }

    // Ctrl+N / Ctrl+P: the next / previous difference section
    func jumpSection(_ dir: Int) {
        guard let s = session else { return }
        commitEditor()
        let m = s.modelRow(s.cursor)
        let si = dir > 0 ? s.model.nextSection(after: m) : s.model.prevSection(before: s.model.section(at: m).map { s.model.sections[$0].rows.lowerBound } ?? m)
        guard let si else {
            s.status = dir > 0 ? "no more differences below" : "no more differences above"
            syncAll()
            return
        }
        s.status = ""
        s.anchor = nil
        s.cursor = s.displayRow(s.model.sections[si].rows.lowerBound)
        scrollCursorVisible(center: true)
        syncAll()
    }

    // the rows Opt+→ / Opt+← (Ctrl+R) act on: a row selection, else the cursor's section
    private func targetRows(_ s: CompareSession) -> Range<Int>? {
        if let a = s.anchor {
            let lo = s.modelRow(min(a, s.cursor)), hi = s.modelRow(max(a, s.cursor))
            return lo..<(hi + 1)
        }
        let m = s.modelRow(s.cursor)
        if let si = s.model.section(at: m) { return s.model.sections[si].rows }
        return nil
    }

    // Opt+→ / Ctrl+R (from: left) / Opt+← (from: right)
    func copyAcross(from: CompareSide) {
        guard let s = session else { return }
        commitEditor()
        guard let rows = targetRows(s) else {
            s.status = "nothing to copy here — the cursor is on a line that is the same"
            syncAll()
            return
        }
        s.willEdit(from.other)
        let cursorModel = rows.lowerBound
        if s.model.copyRows(rows, from: from) != nil {
            s.anchor = nil
            s.status = "copied to the \(from.other.rawValue)"
            modelChanged(s, keepCursor: true)
            s.cursor = s.displayRow(min(cursorModel, max(0, s.model.rows.count - 1)))
            syncAll()
        }
    }

    // Ctrl+Opt+R / Ctrl+Opt+L: just the cursor's line
    func copyLine(from: CompareSide) {
        guard let s = session, s.displayCount > 0 else { return }
        commitEditor()
        let m = s.modelRow(s.cursor)
        s.willEdit(from.other)
        if s.model.copyRows(m..<(m + 1), from: from) != nil {
            s.status = "line copied to the \(from.other.rawValue)"
            modelChanged(s, keepCursor: true)
        }
    }

    func setFilter(_ f: CompareFilter) {
        guard let s = session else { return }
        commitEditor()
        let m = s.displayCount > 0 ? s.modelRow(s.cursor) : 0
        s.filter = f
        s.anchor = nil
        s.refresh(context: cfg.contextLines)
        s.cursor = s.displayRow(m)
        sizePane()
        scrollCursorVisible(center: true)
        syncAll()
    }

    func swapSides() {
        guard let s = session else { return }
        if s.folder != nil { folderPage.swapSides(); return }
        commitEditor()
        s.model.swapSides()
        s.path = [.left: s.path[.right], .right: s.path[.left]].compactMapValues { $0 }
        s.title = [.left: s.title[.right], .right: s.title[.left]].compactMapValues { $0 }
        s.disk = [.left: s.disk[.right], .right: s.disk[.left]].compactMapValues { $0 }
        s.binary = [.left: s.binary[.right], .right: s.binary[.left]].compactMapValues { $0 }
        s.tooLarge = [.left: s.tooLarge[.right], .right: s.tooLarge[.left]].compactMapValues { $0 }
        s.cleanDepth = [.left: s.cleanDepth[.right] ?? 0, .right: s.cleanDepth[.left] ?? 0]
        s.changedOnDisk = [:]
        s.stopWatching()
        for side in [CompareSide.left, .right] where s.path[side] != nil { watch(s, side) }
        s.focus = s.focus.other
        modelChanged(s, keepCursor: true)
    }

    // F5: both files read again (an edited side asks nothing: its edits stay
    // in the undo stack only if it isn't reloaded — so edited sides are kept)
    func reload() {
        guard let s = session else { return }
        if s.folder != nil { folderPage.rescan(); return }
        commitEditor()
        var kept: [String] = []
        for side in [CompareSide.left, .right] {
            guard let p = s.path[side] else { continue }
            if s.dirty(side) { kept.append(side.rawValue); continue }
            let t = load(p, into: s, side: side) ?? TextSide()
            s.model.left = side == .left ? t : s.model.left
            s.model.right = side == .right ? t : s.model.right
            s.changedOnDisk[side] = nil
        }
        s.model.recompute()
        s.status = kept.isEmpty ? "reloaded" : "reloaded (kept your edits on the \(kept.joined(separator: " + ")) side)"
        modelChanged(s, keepCursor: true)
        updateBanner()
    }

    func undo(redo: Bool = false) {
        guard let s = session else { return }
        if s.folder != nil { if !redo { folderPage.undo() }; return }
        commitEditor()
        let side = redo ? nil : (s.model.undoStack.contains { $0.0 == s.focus } ? s.focus : nil)
        let e = redo ? s.model.redo() : s.model.undo(side)
        guard e != nil else { s.status = redo ? "nothing to redo" : "nothing to undo"; syncAll(); return }
        s.status = redo ? "redone" : "undone"
        modelChanged(s, keepCursor: true)
    }

    // MARK: the section editor

    // Return / typing / double-click: an NSTextView over the cursor's section
    // (or line) on the focused side
    func beginEdit(typing: String? = nil) {
        guard let s = session, !s.isBinary else { return }
        commitEditor()
        if s.displayCount == 0 {
            // an empty pair: one empty line to type into
        }
        let m = s.displayCount > 0 ? s.modelRow(s.cursor) : 0
        let rows: Range<Int>
        if s.displayCount == 0 {
            rows = 0..<0
        } else if let si = s.model.section(at: m), s.filter != .same {
            rows = s.model.sections[si].rows
        } else {
            rows = m..<(m + 1)
        }
        let lines = s.displayCount == 0 ? 0..<0 : s.model.lineRange(s.focus, rows: rows)
        let side = s.model.side(s.focus)
        let text = side.lines[lines].map { $0 + "\n" }.joined()
        let ed = CompareEditor(frame: .zero)
        ed.side = s.focus
        ed.rows = rows
        ed.lines = lines
        ed.original = text
        ed.isRichText = false
        ed.allowsUndo = true
        ed.font = pane.font
        ed.textColor = colors.text
        ed.backgroundColor = colors.surface0
        ed.insertionPointColor = colors.accentOn
        ed.drawsBackground = true
        ed.borderColor = colors.accentOn
        ed.textContainerInset = NSSize(width: 0, height: 2)
        ed.textContainer?.lineFragmentPadding = 0
        ed.textContainer?.widthTracksTextView = true
        ed.isAutomaticQuoteSubstitutionEnabled = false
        ed.isAutomaticDashSubstitutionEnabled = false
        ed.isAutomaticTextReplacementEnabled = false
        ed.isAutomaticSpellingCorrectionEnabled = false
        ed.isContinuousSpellCheckingEnabled = false
        ed.smartInsertDeleteEnabled = false
        let p = NSMutableParagraphStyle()
        p.minimumLineHeight = pane.rowH
        p.maximumLineHeight = pane.rowH
        p.defaultTabInterval = pane.charW * CGFloat(cfg.tabWidth)
        p.tabStops = []
        let natural = pane.font.ascender - pane.font.descender
        ed.defaultParagraphStyle = p
        ed.typingAttributes = [.font: pane.font, .foregroundColor: colors.text, .paragraphStyle: p,
                               .baselineOffset: max(0, (pane.rowH - natural) / 2 - 1)]
        ed.string = text
        ed.textStorage?.setAttributes(ed.typingAttributes, range: NSRange(location: 0, length: (text as NSString).length))
        ed.delegate = self
        ed.onResize = { [weak self] in self?.pane.needsDisplay = true }
        editor = ed
        pane.addSubview(ed)
        positionEditor()
        ed.fit()
        // the caret: the cursor's line at the clicked column
        let ns = text as NSString
        var caret = 0
        if s.displayCount > 0 {
            let curLine = s.model.rows[m].line(s.focus)
            if curLine >= 0 {
                for i in lines.lowerBound..<curLine { caret += (side.lines[i] as NSString).length + 1 }
                let lineText = side.lines[curLine]
                var vis = 0, idx = 0
                for ch in lineText.utf16 {
                    if vis >= s.col { break }
                    vis += ch == 9 ? cfg.tabWidth - vis % cfg.tabWidth : 1
                    idx += 1
                }
                caret += idx
            }
        }
        ed.setSelectedRange(NSRange(location: min(caret, ns.length), length: 0))
        window.makeFirstResponder(ed)
        if let typing { ed.insertText(typing, replacementRange: ed.selectedRange()) }
        s.status = "editing the \(s.focus.rawValue) side — Esc or click away to finish"
        syncAll()
    }

    private func positionEditor() {
        guard let ed = editor, let s = session else { return }
        let first = s.displayCount == 0 ? 0 : s.displayRow(ed.rows.lowerBound)
        let count = max(1, ed.rows.count)
        let r = pane.textRect(ed.side, row: first, rows: count)
        ed.minHeight = r.height
        ed.frame = NSRect(x: r.minX - 2, y: r.minY, width: r.width + 4, height: max(ed.frame.height, r.height))
        ed.fit()
    }

    // the editor's text back into the model (one undo step); true = it was open
    @discardableResult
    func commitEditor() -> Bool {
        guard let ed = editor else { return false }
        editor = nil
        let text = ed.string
        ed.removeFromSuperview()
        if window.firstResponder === ed || window.firstResponder == nil { window.makeFirstResponder(pane) }
        guard let s = session else { return true }
        if text != ed.original {
            let t0 = DispatchTime.now().uptimeNanoseconds
            s.willEdit(ed.side)
            s.model.replace(ed.side, ed.lines, with: ed.editedLines)
            let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
            controller?.log(String(format: "compare edit: %d lines, re-diff %.1f ms", s.model.rows.count, ms))
            s.status = "edited"
            modelChanged(s, keepCursor: true)
        } else {
            s.status = ""
            syncAll()
        }
        return true
    }

    // MARK: save

    // Cmd+S: the focused side; a side without a file asks where (a sheet)
    func save(_ side: CompareSide, then: ((Bool) -> Void)? = nil) {
        guard let s = session else { then?(false); return }
        save(s, side, then: then)
    }

    private func save(_ s: CompareSession, _ side: CompareSide, then: ((Bool) -> Void)? = nil) {
        commitEditor()
        guard let p = s.path[side] else {
            let panel = NSSavePanel()
            panel.nameFieldStringValue = "\(side.rawValue).txt"
            panel.beginSheetModal(for: window) { [weak self] r in
                guard r == .OK, let u = panel.url, let self else { then?(false); return }
                s.path[side] = u.path
                self.save(s, side, then: then)
                self.watch(s, side)
            }
            return
        }
        var t = s.model.side(side)
        var note = ""
        var data = t.encoded()
        if data == nil {
            t.encoding = .utf8
            data = t.encoded()
            note = " (as UTF-8: \(s.model.side(side).encoding.rawValue) can't hold every character)"
            if side == .left { s.model.left.encoding = .utf8 } else { s.model.right.encoding = .utf8 }
        }
        guard let data else { then?(false); return }
        // write the link's target, keep its permissions
        let target = URL(fileURLWithPath: p).resolvingSymlinksInPath()
        let perms = (try? FileManager.default.attributesOfItem(atPath: target.path))?[.posixPermissions]
        do {
            s.disk[side] = data
            try data.write(to: target, options: .atomic)
            if let perms { try? FileManager.default.setAttributes([.posixPermissions: perms], ofItemAtPath: target.path) }
            s.markClean(side)
            s.changedOnDisk[side] = nil
            s.status = "saved \(tilde(p))" + note
            FileDrag.onFileOp?(nil, target.path)
            // atomic writes replace the inode: watch the new one
            watch(s, side)
            updateBanner()
            syncAll()
            then?(true)
        } catch {
            s.status = "save failed: \(error.localizedDescription)"
            syncAll()
            then?(false)
        }
    }

    // both dirty sides (Cmd+Opt+S, the close sheet's Save)
    private func saveAll(_ s: CompareSession, then: @escaping (Bool) -> Void) {
        let sides = [CompareSide.left, .right].filter { s.dirty($0) }
        func next(_ rest: ArraySlice<CompareSide>) {
            guard let side = rest.first else { then(true); return }
            save(s, side) { ok in ok ? next(rest.dropFirst()) : then(false) }
        }
        next(sides[...])
    }

    // MARK: changed on disk

    private func watch(_ s: CompareSession, _ side: CompareSide) {
        s.watchers[side]?.cancel()
        s.watchers[side] = nil
        guard let p = s.path[side] else { return }
        let fd = open(p, O_EVTONLY)
        guard fd >= 0 else { return }
        let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .delete, .rename, .extend],
                                                            queue: .main)
        src.setEventHandler { [weak self, weak s] in
            guard let self, let s else { return }
            let ev = src.data
            if ev.contains(.delete) || ev.contains(.rename) {
                // an editor's atomic save swaps the file: look again shortly
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self, weak s] in
                    guard let self, let s else { return }
                    self.diskChanged(s, side)
                    self.watch(s, side)
                }
            } else {
                self.diskChanged(s, side)
            }
        }
        src.setCancelHandler { close(fd) }
        src.resume()
        s.watchers[side] = src
    }

    // not edited here → reload silently; edited here → the banner asks
    private func diskChanged(_ s: CompareSession, _ side: CompareSide) {
        guard let p = s.path[side], let d = FileManager.default.contents(atPath: p), d != s.disk[side] else { return }
        if s.dirty(side) {
            s.changedOnDisk[side] = true
            if s === session { updateBanner(); syncAll() }
            return
        }
        let t = load(p, into: s, side: side) ?? TextSide()
        s.model.setSide(side, t)
        s.markClean(side)
        s.status = "\((p as NSString).lastPathComponent) changed on disk — reloaded"
        if s === session { modelChanged(s, keepCursor: true) } else { s.refresh(context: cfg.contextLines) }
    }

    private func updateBanner() {
        guard let s = session, let side = [CompareSide.left, .right].first(where: { s.changedOnDisk[$0] == true }) else {
            if !banner.isHidden { banner.isHidden = true; layoutText(body.bounds) }
            bannerSide = nil
            return
        }
        bannerSide = side
        bannerText.stringValue = cfg.label("changed", "{} changed on disk — and you have edits here", s.name(side))
        bannerReload.title = cfg.label("reload-button", "Reload")
        bannerKeep.title = cfg.label("keep-button", "Keep Mine")
        if banner.isHidden { banner.isHidden = false; layoutText(body.bounds) }
    }

    private func resolveBanner(keep: Bool) {
        guard let s = session, let side = bannerSide else { return }
        if !keep, s.dirty(side), s.path[side] != nil {
            // Reload throws the side's edits away: ask first
            askDiscard(s, side, what: cfg.label("reload-info", "Reloading brings back the file as it is on disk.")) { [weak self] in
                self?.applyBanner(keep: false)
            }
            return
        }
        applyBanner(keep: keep)
    }

    private func applyBanner(keep: Bool) {
        guard let s = session, let side = bannerSide else { return }
        s.changedOnDisk[side] = nil
        if !keep, let p = s.path[side] {
            let t = load(p, into: s, side: side) ?? TextSide()
            s.model.setSide(side, t)
            s.markClean(side)
            modelChanged(s, keepCursor: true)
        } else if keep {
            // "mine" wins: saving later overwrites the disk version
            s.disk[side] = FileManager.default.contents(atPath: s.path[side] ?? "")
        }
        updateBanner()
        syncAll()
    }

    // MARK: open into a side, paste

    func openPanel(into side: CompareSide) {
        let p = NSOpenPanel()
        p.canChooseFiles = true
        p.canChooseDirectories = false
        p.beginSheetModal(for: window) { [weak self] r in
            guard r == .OK, let u = p.url else { return }
            self?.openFile(u.path, into: side)
        }
    }

    private func pasteboardText() -> String? {
        let pb = NSPasteboard.general
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
           let u = urls.first { return "file:" + u.path }
        return pb.string(forType: .string)
    }

    // the clipboard into a side: a copied FILE opens it, text is pasted text
    func pasteClipboard(into side: CompareSide) {
        guard let t = pasteboardText() else { return }
        if t.hasPrefix("file:") {
            let p = String(t.dropFirst(5))
            if session == nil { openPair(side == .left ? p : nil, side == .right ? p : nil) } else { openFile(p, into: side) }
        } else {
            setPasted(t, side)
        }
    }

    // MARK: menus

    private func pillMenu(_ i: Int) -> NSMenu? {
        guard sessions.indices.contains(i) else { return nil }
        let s = sessions[i]
        let m = NSMenu()
        m.addItem(menuItem("Close") { [weak self] in self?.closeSession(i) })
        m.addItem(.separator())
        for side in [CompareSide.left, .right] {
            guard let p = s.path[side] else { continue }
            m.addItem(menuItem("Copy \(side == .left ? "Left" : "Right") Path") { copyText(p) })
            m.addItem(menuItem("Reveal \(side == .left ? "Left" : "Right") in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)])
            })
        }
        return m
    }

    // the text pane's right-click menu (rule 2)
    func paneMenu(row: Int, side: CompareSide) -> NSMenu? {
        guard let s = session else { return nil }
        let m = NSMenu()
        m.autoenablesItems = false
        let other = side == .left ? "Right" : "Left"
        m.addItem(menuItem("Copy") { [weak self] in self?.copyRowsText() })
        m.addItem(menuItem("Copy to \(other)  (⌃\(side == .left ? "R" : "L"))", enabled: targetRows(s) != nil) { [weak self] in
            self?.copyAcross(from: side)
        })
        m.addItem(menuItem("Copy Line to \(other)  (⌃⌥\(side == .left ? "R" : "L"))", enabled: s.displayCount > 0) { [weak self] in
            self?.copyLine(from: side)
        })
        m.addItem(menuItem("Paste Clipboard Here", enabled: pasteboardText() != nil) { [weak self] in self?.pasteClipboard(into: side) })
        m.addItem(menuItem("Edit Section  (⏎)", enabled: s.displayCount > 0) { [weak self] in self?.beginEdit() })
        m.addItem(menuItem("Select Section", enabled: s.displayCount > 0 && s.model.section(at: s.modelRow(s.cursor)) != nil) { [weak self] in
            self?.selectSection()
        })
        m.addItem(menuItem("Find…  (⌘F)") { [weak self] in self?.openFind(.find) })
        m.addItem(.separator())
        // Align With (BC): pick a line on one side, then its partner on the other
        let mrow = s.displayCount > 0 ? s.modelRow(row) : -1
        let line = mrow >= 0 ? s.model.rows[mrow].line(side) : -1
        if let pick = alignPick, pick.side != side, line >= 0 {
            m.addItem(menuItem("Align With Picked \(pick.side == .left ? "Left" : "Right") Line \(pick.line + 1)") { [weak self] in
                self?.alignWith(side: side, line: line)
            })
            m.addItem(menuItem("Cancel Align With") { [weak self] in self?.alignPick = nil; self?.pane.needsDisplay = true })
        } else {
            m.addItem(menuItem("Align With…  (pick this line, then one on the other side)", enabled: line >= 0) { [weak self] in
                self?.alignWith(side: side, line: line)
            })
        }
        if s.model.isAnchor(row: mrow) {
            m.addItem(menuItem("Remove This Alignment") { [weak self] in self?.clearAlignment(row: mrow) })
        }
        if !s.model.anchors.isEmpty {
            m.addItem(menuItem("Clear All Alignments (\(s.model.anchors.count))") { [weak self] in self?.clearAlignment(row: nil) })
        }
        m.addItem(convertMenuItem(side))
        m.addItem(.separator())
        let path = s.path[side]
        m.addItem(menuItem("Copy Path", enabled: path != nil) { if let path { copyText(path) } })
        m.addItem(menuItem("Reveal in Finder", enabled: path != nil) {
            if let path { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
        })
        m.addItem(menuItem("Open in Notes", enabled: path != nil) { [weak self] in if let path { self?.controller?.openNoteFile(path) } })
        m.addItem(menuItem("Open in Default App", enabled: path != nil) { if let path { NSWorkspace.shared.open(URL(fileURLWithPath: path)) } })
        m.addItem(menuItem("Open File into This Side…  (⌘O)") { [weak self] in self?.openPanel(into: side) })
        m.addItem(menuItem("Save This Side  (⌘S)", enabled: s.dirty(side) || path == nil) { [weak self] in
            guard let self, let s = self.session else { return }
            self.save(s, side)
        })
        return m
    }

    // MARK: Align With, Convert (phase 3)

    // the first call picks a line; a call on the other side aligns the two
    func alignWith(side: CompareSide, line: Int) {
        guard let s = session, line >= 0 else { return }
        if let pick = alignPick, pick.side != side {
            alignPick = nil
            commitEditor()
            let l = side == .left ? line : pick.line, r = side == .left ? pick.line : line
            s.model.align(left: l, right: r)
            s.status = "aligned left \(l + 1) with right \(r + 1)"
            modelChanged(s, keepCursor: true)
            if let row = s.model.rows.firstIndex(where: { Int($0.l) == l && Int($0.r) == r }) { moveCursor(s.displayRow(row)) }
            return
        }
        alignPick = (side, line)
        s.status = "Align With: now pick a line on the \(side.other.rawValue) side (right-click ▸ Align With Picked…)"
        pane.needsDisplay = true
        syncAll()
    }

    func clearAlignment(row: Int?) {
        guard let s = session else { return }
        commitEditor()
        alignPick = nil
        s.model.clearAlignment(row: row)
        s.status = row == nil ? "alignments cleared" : "alignment removed"
        modelChanged(s, keepCursor: true)
    }

    func trimTrailing(_ side: CompareSide) {
        guard let s = session, s.binary[side] == nil, s.tooLarge[side] == nil else { return }
        commitEditor()
        s.willEdit(side)
        let n = s.model.trimTrailingWhitespace(side)
        s.status = n == 0 ? "no trailing whitespace on the \(side.rawValue) side" : "trimmed \(n) line\(n == 1 ? "" : "s") on the \(side.rawValue) (⌘Z undoes)"
        modelChanged(s, keepCursor: true)
    }

    func convertEOL(_ side: CompareSide, _ eol: EOL) {
        guard let s = session, s.binary[side] == nil, s.tooLarge[side] == nil else { return }
        commitEditor()
        s.willEdit(side)
        let n = s.model.convertLineEndings(side, to: eol)
        s.status = n == 0 ? "the \(side.rawValue) side already ends lines with \(eol.label)"
            : "converted \(n) line ending\(n == 1 ? "" : "s") to \(eol.label) on the \(side.rawValue) (⌘Z undoes)"
        modelChanged(s, keepCursor: true)
    }

    private func convertMenuItem(_ side: CompareSide) -> NSMenuItem {
        let sub = NSMenu()
        sub.addItem(menuItem("Trim Trailing Whitespace") { [weak self] in self?.trimTrailing(side) })
        sub.addItem(.separator())
        for eol in [EOL.lf, .crlf, .cr] {
            sub.addItem(menuItem("Line Endings → \(eol.label)") { [weak self] in self?.convertEOL(side, eol) })
        }
        let it = NSMenuItem(title: "Convert \(side == .left ? "Left" : "Right") Side", action: nil, keyEquivalent: "")
        it.submenu = sub
        return it
    }

    private func toggleWhitespace() {
        showWhitespace.toggle()
        UserDefaults.standard.set(showWhitespace, forKey: "compareWhitespace")
        pane.needsDisplay = true
    }

    private func selectSection() {
        guard let s = session, let si = s.model.section(at: s.modelRow(s.cursor)) else { return }
        let r = s.model.sections[si].rows
        s.anchor = s.displayRow(r.lowerBound)
        s.cursor = s.displayRow(r.upperBound - 1)
        syncAll()
    }

    // Cmd+C: the focused side's lines of the selection (else the cursor's line)
    private func copyRowsText() {
        guard let s = session, s.displayCount > 0 else { return }
        let lo = min(s.anchor ?? s.cursor, s.cursor), hi = max(s.anchor ?? s.cursor, s.cursor)
        var out: [String] = []
        for d in lo...hi {
            let line = s.model.rows[s.modelRow(d)].line(s.focus)
            if line >= 0 { out.append(s.model.side(s.focus).lines[line]) }
        }
        copyText(out.joined(separator: "\n") + (out.count > 1 ? "\n" : ""))
        s.status = "copied \(out.count) line\(out.count == 1 ? "" : "s")"
        syncAll()
    }

    // ⚙: importance switches for this session; "Save as Default" writes [compare]
    private func showImportanceMenu() {
        guard let s = session else { return }
        let m = NSMenu()
        let imp = s.model.importance
        func toggle(_ title: String, _ on: Bool, _ set: @escaping (inout Importance) -> Void) {
            m.addItem(menuItem(title, state: on) { [weak self] in
                guard let self, let s = self.session else { return }
                self.commitEditor()
                var i = s.model.importance
                set(&i)
                s.model.setImportance(i)
                self.modelChanged(s, keepCursor: true)
            })
        }
        m.addItem(NSMenuItem.sectionHeader(title: "Unimportant (blue) differences"))
        toggle("Leading Whitespace", imp.leadingWS) { $0.leadingWS.toggle() }
        toggle("Trailing Whitespace", imp.trailingWS) { $0.trailingWS.toggle() }
        toggle("Embedded Whitespace", imp.embeddedWS) { $0.embeddedWS.toggle() }
        toggle("Character Case", imp.ignoreCase) { $0.ignoreCase.toggle() }
        toggle("Line Endings (CRLF / LF)", imp.lineEndings) { $0.lineEndings.toggle() }
        toggle("Blank Lines", imp.blankLines) { $0.blankLines.toggle() }
        m.addItem(.separator())
        m.addItem(menuItem("Ignore Unimportant Differences", state: s.model.ignoreUnimportant) { [weak self] in
            guard let self, let s = self.session else { return }
            s.model.setIgnoreUnimportant(!s.model.ignoreUnimportant)
            self.modelChanged(s, keepCursor: true)
        })
        m.addItem(.separator())
        m.addItem(menuItem("Save as Default") { [weak self] in
            guard let self, let s = self.session else { return }
            let i = s.model.importance
            let b = { (v: Bool) in v ? "true" : "false" }
            saveConfigValues(section: "compare", [
                ("ignore-leading-ws", b(i.leadingWS)), ("ignore-trailing-ws", b(i.trailingWS)),
                ("ignore-embedded-ws", b(i.embeddedWS)), ("ignore-case", b(i.ignoreCase)),
                ("ignore-line-endings", b(i.lineEndings)), ("ignore-blank-lines", b(i.blankLines)),
            ])
            s.status = "importance saved to [compare]"
            self.syncAll()
        })
        let b = toolButtons[4]
        m.popUp(positioning: nil, at: NSPoint(x: b.frame.minX, y: b.frame.maxY + 4), in: toolbar)
    }

    // Cmd+K: every action of the view by name (type to jump, ⏎ runs it)
    private func actionPicker() {
        let m = NSMenu()
        for (title, f) in actions() { m.addItem(menuItem(title, f)) }
        let host = session?.folder != nil ? folderPage.root : body
        let p = NSPoint(x: host.bounds.midX - 120, y: host.bounds.minY + 60)
        m.popUp(positioning: nil, at: p, in: host)
    }

    private func actions() -> [(String, () -> Void)] {
        var a: [(String, () -> Void)] = [("New Comparison (start page)", { [weak self] in self?.showStartPage() })]
        guard session != nil else { return a }
        if session?.folder != nil {
            let f = folderPage
            return a + [
                ("Next Difference  ⌃N", { f.jumpDiff(1) }),
                ("Previous Difference  ⌃P", { f.jumpDiff(-1) }),
                ("Open Pair (Text Compare)  ⏎", { if let s = f.session { f.activate(s.cursor) } }),
                ("Copy to Right  ⌥→", { f.copy(from: .left) }),
                ("Copy to Left  ⌥←", { f.copy(from: .right) }),
                ("Move to Right  ⌃⌥R", { f.transfer(from: .left, move: true) }),
                ("Move to Left  ⌃⌥L", { f.transfer(from: .right, move: true) }),
                ("Rename…  F2", { f.beginRename() }),
                ("New Folder…  ⌘⇧N", { f.newFolder() }),
                ("Move to Trash  ⌘⌫", { f.trash() }),
                ("Undo  ⌘Z", { f.undo() }),
                ("Show All  ⌘1", { f.setFilter(.all) }),
                ("Show Differences  ⌘2", { f.setFilter(.diffs) }),
                ("Show Same  ⌘3", { f.setFilter(.same) }),
                ("Show Orphans  ⌘4", { f.setFilter(.orphans) }),
                ("Show Left Newer  ⌘5", { f.setFilter(.leftNewer) }),
                ("Show Right Newer  ⌘6", { f.setFilter(.rightNewer) }),
                ("Ignore Folder Structure (flatten)", { f.toggleFlatten() }),
                ("Filter Names  ⌘F", { f.focusNameBox() }),
                ("Swap Sides  ⌘⌥X", { f.swapSides() }),
                ("Scan Again  F5", { f.rescan() }),
                ("Quick Look  Space", { f.toggleQuickLook() }),
                ("Set as Base Folder  ⌘↓", { if let s = f.session { f.setBase(s.cursor, side: nil) } }),
                ("Up One Level  ⌘↑", { f.upOneLevel() }),
                ("Back  ⌘[", { f.goBack() }),
                ("Forward  ⌘]", { f.goForward() }),
            ] + SyncMode.allCases.map { m in ("Synchronize: \(m.title)…", { f.previewSync(m) }) } + [
                ("Close Session", { [weak self] in if let i = self?.selected { self?.closeSession(i) } }),
                ("Keyboard Shortcuts  ⌘/", { [weak self] in self?.showShortcuts() }),
            ]
        }
        a += [
            ("Next Difference  ⌃N", { [weak self] in self?.jumpSection(1) }),
            ("Previous Difference  ⌃P", { [weak self] in self?.jumpSection(-1) }),
            ("Copy to Right  ⌥→", { [weak self] in self?.copyAcross(from: .left) }),
            ("Copy to Left  ⌥←", { [weak self] in self?.copyAcross(from: .right) }),
            ("Copy Line to Right  ⌃⌥R", { [weak self] in self?.copyLine(from: .left) }),
            ("Copy Line to Left  ⌃⌥L", { [weak self] in self?.copyLine(from: .right) }),
            ("Edit Section  ⏎", { [weak self] in self?.beginEdit() }),
            ("Undo  ⌘Z", { [weak self] in self?.undo() }),
            ("Redo  ⌘⇧Z", { [weak self] in self?.undo(redo: true) }),
            ("Save Focused Side  ⌘S", { [weak self] in if let s = self?.session { self?.save(s.focus) } }),
            ("Save Both  ⌘⌥S", { [weak self] in if let s = self?.session { self?.saveAll(s) { _ in } } }),
            ("Open File into Focused Side…  ⌘O", { [weak self] in if let s = self?.session { self?.openPanel(into: s.focus) } }),
            ("Paste Clipboard into Focused Side", { [weak self] in if let s = self?.session { self?.pasteClipboard(into: s.focus) } }),
            ("Show All  ⌘1", { [weak self] in self?.setFilter(.all) }),
            ("Show Differences  ⌘2", { [weak self] in self?.setFilter(.diffs) }),
            ("Show Same  ⌘3", { [weak self] in self?.setFilter(.same) }),
            ("Show Context  ⌘4", { [weak self] in self?.setFilter(.context) }),
            ("Find  ⌘F", { [weak self] in self?.openFind(.find) }),
            ("Go to Line  ⌃G", { [weak self] in self?.openFind(.goTo) }),
            ("Swap Sides  ⌘⌥X", { [weak self] in self?.swapSides() }),
            ("Reload  F5", { [weak self] in self?.reload() }),
            ("Toggle Line Details", { [weak self] in self?.toggleDetails() }),
            ("Show Whitespace (toggle)", { [weak self] in self?.toggleWhitespace() }),
            ("Align With… (pick the cursor line, then one on the other side)", { [weak self] in
                guard let self, let s = self.session, s.displayCount > 0 else { return }
                self.alignWith(side: s.focus, line: s.model.rows[s.modelRow(s.cursor)].line(s.focus))
            }),
            ("Clear All Alignments", { [weak self] in self?.clearAlignment(row: nil) }),
            ("Trim Trailing Whitespace (Focused Side)", { [weak self] in if let s = self?.session { self?.trimTrailing(s.focus) } }),
            ("Convert Line Endings to LF (Focused Side)", { [weak self] in if let s = self?.session { self?.convertEOL(s.focus, .lf) } }),
            ("Convert Line Endings to CRLF (Focused Side)", { [weak self] in if let s = self?.session { self?.convertEOL(s.focus, .crlf) } }),
            ("Close Session", { [weak self] in if let i = self?.selected { self?.closeSession(i) } }),
            ("Keyboard Shortcuts  ⌘/", { [weak self] in self?.showShortcuts() }),
        ]
        return a
    }

    private func toggleDetails() {
        showDetails.toggle()
        UserDefaults.standard.set(showDetails, forKey: "compareDetails")
        layoutText(body.bounds)
    }

    // the kitchen sink
    override func showIconMenu() {
        let menu = iconMenu(view: isSub ? .compareText : .compare)
        func add(_ t: String, enabled: Bool = true, state: Bool? = nil, _ f: @escaping () -> Void) {
            menu.addItem(menuItem(t, state: state, enabled: enabled, f))
        }
        if !isSub { add("New Comparison") { [weak self] in self?.showStartPage() } }
        let has = session != nil
        add("Open Left File…", enabled: has) { [weak self] in self?.openPanel(into: .left) }
        add("Open Right File…", enabled: has) { [weak self] in self?.openPanel(into: .right) }
        add("Paste Clipboard as Left") { [weak self] in self?.pasteClipboard(into: .left) }
        add("Paste Clipboard as Right") { [weak self] in self?.pasteClipboard(into: .right) }
        menu.addItem(.separator())
        add("Swap Sides", enabled: has) { [weak self] in self?.swapSides() }
        add("Reload", enabled: has) { [weak self] in self?.reload() }
        add("Save Both", enabled: session?.isDirty == true) { [weak self] in if let s = self?.session { self?.saveAll(s) { _ in } } }
        menu.addItem(.separator())
        add("Show Line Details", state: showDetails) { [weak self] in self?.toggleDetails() }
        add("Show Whitespace", state: showWhitespace) { [weak self] in self?.toggleWhitespace() }
        if let s = session, s.folder == nil {
            menu.addItem(convertMenuItem(.left))
            menu.addItem(convertMenuItem(.right))
            add("Clear All Alignments", enabled: !s.model.anchors.isEmpty) { [weak self] in self?.clearAlignment(row: nil) }
        }
        if let f = session?.folder {
            let sync = NSMenu()
            for m in SyncMode.allCases {
                sync.addItem(menuItem(m.title + "…") { [weak self] in self?.folderPage.previewSync(m) })
            }
            let si = NSMenuItem(title: "Synchronize", action: nil, keyEquivalent: "")
            si.submenu = sync
            menu.addItem(si)
            add("Back", enabled: !f.back.isEmpty) { [weak self] in self?.folderPage.goBack() }
            add("Forward", enabled: !f.forward.isEmpty) { [weak self] in self?.folderPage.goForward() }
            add("Up One Level") { [weak self] in self?.folderPage.upOneLevel() }
        }
        let arrows = NSMenu()
        for mode in ["hover", "always", "off"] {
            arrows.addItem(menuItem(mode.capitalized, state: cfg.gutterArrows == mode) { [weak self] in
                saveConfigValues(section: "compare", [("gutter-arrows", mode)])
                self?.cfg = CompareConfig.load()
                self?.pane.needsDisplay = true
            })
        }
        let ai = NSMenuItem(title: "Gutter Arrows", action: nil, keyEquivalent: "")
        ai.submenu = arrows
        menu.addItem(ai)
        menu.addItem(.separator())
        add("Close Session", enabled: has) { [weak self] in if let i = self?.selected { self?.closeSession(i) } }
        add("Keyboard Shortcuts…") { [weak self] in self?.showShortcuts() }
        popUpIconMenu(menu)
    }

    // commands.toml [shortcuts] as the shared themed card: the mode you are
    // in first ("compare: …" Text, "compare-folders: …" Folder), the other
    // mode next, then "all: …"
    private func showShortcuts() {
        func items(_ v: String) -> [(keys: String, what: String)] {
            shortcutEntries.filter { $0.view == v }.map { ($0.keys, $0.what) }
        }
        let text: ShortcutsOverlay.Group = ("Text Compare", items("compare"))
        let folder: ShortcutsOverlay.Group = ("Folder Compare", items("compare-folders"))
        let modes = session?.folder != nil ? [folder, text] : [text, folder]
        var groups = modes + sharedShortcutGroups()
        if !groups.contains(where: { !$0.items.isEmpty }) {
            groups = [("Compare", [("Cmd+/", "add \"compare: keys\" = \"what\" lines to [shortcuts] in commands.toml")])]
        }
        showShortcutsCard(groups, colors: colors, title: cfg.label("shortcuts-title", "Compare Shortcuts"))
    }

    // MARK: keys (CardWindowController's monitor: sheets, Ctrl+Tab, Cmd+W first)

    // Esc, first match wins (PRD §7.1): (1) a find bar / path field / the
    // section editor closes; (2) a running diff stops; (3) the pushed text
    // compare steps back; (4) at the top: hide only with "Esc Hides Window"
    private func escape() {
        if findMode != .none { closeFind(); return }
        if pathEditSide != nil { cancelPathEdit(); return }
        if session?.folder != nil, folderPage.escape() { return }
        if commitEditor() { session?.status = ""; syncAll(); return }
        if diffRunning {
            diffGen += 1
            diffRunning = false
            session?.status = "compare stopped"
            syncAll()
            return
        }
        if let s = session, s.anchor != nil { s.anchor = nil; syncAll(); return }
        guard let c = controller, onSlotHide != nil else { return }
        if isSub { c.slot.back(esc: true) } else { c.slot.escapeAtTop(.compare) }
    }

    override func handleKey(_ e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        let opt = mods.contains(.option), shift = mods.contains(.shift)
        let code = e.keyCode
        if code == 53 { escape(); return true }                                   // Esc
        if cmd && code == 44 { showShortcuts(); return true }                     // Cmd+/
        if cmd && code == 40 { actionPicker(); return true }                      // Cmd+K
        if cmd && !shift && code == 29, !isSub { showStartPage(); return true }   // Cmd+0: Home
        if cmd && !shift && code == 45, !isSub, editor == nil { startPasted(); return true }   // Cmd+N: new text compare
        let inText = window.firstResponder is NSText
        let inEditor = editor != nil && window.firstResponder === editor
        // the start page: its fields + the Recent list
        guard let s = session else {
            if ctrl && !cmd && (code == 45 || code == 35) { moveRecent(code == 45 ? 1 : -1); return true }   // Ctrl+N / P
            if cmd && code == 31 { browse(into: window.firstResponder === rightBox.field.currentEditor() ? rightBox : leftBox); return true }
            if cmd && code == 37 { window.makeFirstResponder(leftBox.field); return true }                  // Cmd+L
            if (cmd || ctrl) && code == 9, !inText { pasteClipboard(into: .left); return true }
            if !inText, code == 36 || code == 76 { compareFromStart(); return true }
            if !inText, code == 125 || code == 126 { moveRecent(code == 125 ? 1 : -1); return true }
            return false
        }
        if s.folder != nil {
            if ctrl && !cmd && code == 48 { return false }
            return folderPage.handleKey(e)
        }
        // the section editor: typing is its own; these leave / act on it
        if inEditor {
            if ctrl && !cmd && (code == 45 || code == 35) { jumpSection(code == 45 ? 1 : -1); return true }
            if cmd && code == 1 { commitEditor(); save(s.focus); return true }    // Cmd+S
            return false                                                          // the text view + edit keys
        }
        if findMode != .none, inText {
            if cmd && code == 5 { find(findBox.field.stringValue, dir: shift ? -1 : 1); return true }   // Cmd+G
            return false
        }
        if inText { return false }                                                // a path field: its own keys
        switch code {
        case 45 where ctrl && !cmd: jumpSection(1)                                // Ctrl+N
        case 35 where ctrl && !cmd: jumpSection(-1)                               // Ctrl+P
        // copy across: Opt+→ / Opt+← (WinMerge), Ctrl+R too; plain Ctrl+L is
        // the pane move now (SharedWindow), Ctrl+Opt+L/R copy one line
        case 15 where ctrl && !cmd: opt ? copyLine(from: .left) : copyAcross(from: .left)     // Ctrl+R / Ctrl+Opt+R
        case 37 where ctrl && opt && !cmd: copyLine(from: .right)                             // Ctrl+Opt+L
        case 124 where opt && !cmd && !ctrl: copyAcross(from: .left)                          // Opt+→: to the right
        case 123 where opt && !cmd && !ctrl: copyAcross(from: .right)                         // Opt+←: to the left
        case 5 where ctrl && !cmd: openFind(.goTo)                                // Ctrl+G
        case 6 where cmd: undo(redo: shift)                                       // Cmd+Z / Cmd+Shift+Z
        case 1 where cmd: opt ? saveAll(s) { _ in } : save(s.focus)              // Cmd+S / Cmd+Opt+S
        case 31 where cmd: openPanel(into: s.focus)                               // Cmd+O
        case 37 where cmd: beginPathEdit(s.focus)                                 // Cmd+L
        case 18 where cmd, 19 where cmd, 20 where cmd, 21 where cmd:              // Cmd+1…4
            setFilter(CompareFilter.allCases[[18: 0, 19: 1, 20: 2, 21: 3][Int(code)]!])
        case 3 where cmd: openFind(.find)                                         // Cmd+F
        case 5 where cmd:                                                         // Cmd+G / Cmd+Shift+G
            if !findBox.field.stringValue.isEmpty { find(findBox.field.stringValue, dir: shift ? -1 : 1) }
        case 7 where cmd && opt: swapSides()                                      // Cmd+Opt+X
        case 96: reload()                                                         // F5
        case 8 where cmd || ctrl: copyRowsText()                                  // Cmd+C / Ctrl+C
        case 9 where cmd || ctrl:                                                 // Cmd+V / Ctrl+V: an empty pane takes the paste
            pasteClipboard(into: s.focus)
        case 0 where cmd:                                                         // Cmd+A: every row
            s.anchor = 0
            s.cursor = max(0, s.displayCount - 1)
            syncAll()
        case 48 where !ctrl && !cmd:                                              // Tab: the other side
            s.focus = s.focus.other
            syncAll()
        case 126 where !cmd: moveCursor(s.cursor - 1, extend: shift)              // ↑
        case 125 where !cmd: moveCursor(s.cursor + 1, extend: shift)              // ↓
        case 126 where cmd, 115: moveCursor(0, extend: shift)                     // Cmd+↑ / Home
        case 125 where cmd, 119: moveCursor(s.displayCount - 1, extend: shift)    // Cmd+↓ / End
        case 116: moveCursor(s.cursor - pageRows(), extend: shift)                // PgUp
        case 121: moveCursor(s.cursor + pageRows(), extend: shift)                // PgDn
        case 123 where !cmd && !ctrl:                                             // ← / →: focus a side
            s.focus = .left
            syncAll()
        case 124 where !cmd && !ctrl:
            s.focus = .right
            syncAll()
        case 36, 76: beginEdit()                                                  // Return: edit
        default:
            // typing starts the section editor with that character
            if !cmd && !ctrl, let ch = e.characters, !ch.isEmpty,
               ch.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) && $0.value < 0xF700 }) {
                beginEdit(typing: ch)
                return true
            }
            return false
        }
        return true
    }

    private func pageRows() -> Int { max(1, Int(scroll.contentSize.height / pane.rowH) - 2) }

    // MARK: test hooks (do:compare:… / state compare)

    var testState: [String: Any] {
        var st: [String: Any] = [
            "shown": window.isVisible, "key": window.isKeyWindow, "sub": isSub,
            "startPage": session == nil, "selected": selected ?? -1,
            "sheet": window.attachedSheet != nil || confirmCard != nil,
            "shortcutsCard": shortcutsCard != nil,
            "sessions": sessions.map { s -> [String: Any] in
                ["kind": s.folder != nil ? "folder" : "text", "left": s.path[.left] ?? s.name(.left), "right": s.path[.right] ?? s.name(.right),
                 "dirtyL": s.dirty(.left), "dirtyR": s.dirty(.right), "git": s.git, "waiting": !s.waiters.isEmpty]
            },
            "recent": recentList.rows.count,
        ]
        let f = window.frame
        st["frame"] = [f.origin.x, f.origin.y, f.width, f.height].map { Int($0.rounded()) }
        // the header ✕ in cliclick's coordinates (top-left of the main screen)
        if let ch = chrome, let main = NSScreen.screens.first {
            let r = ch.closeButtonRect
            st["close"] = [Int((f.minX + r.midX).rounded()), Int((main.frame.height - (f.maxY - r.midY)).rounded())]
        }
        if let s = session {
            let m = s.model
            let rows = (0..<min(50, s.displayCount)).map { d -> [String: Any] in
                let r = m.rows[s.modelRow(d)]
                let status = r.kind == .same ? "same" : !m.isDiff(r) ? "same" : r.important ? "important" : "unimportant"
                return ["l": r.l >= 0 ? m.left.lines[Int(r.l)] : NSNull(), "r": r.r >= 0 ? m.right.lines[Int(r.r)] : NSNull(),
                        "status": status, "kind": "\(r.kind)"]
            }
            st["current"] = [
                "sections": m.sections.count, "important": m.importantCount, "unimportant": m.unimportantCount,
                "cursorRow": s.cursor, "focus": s.focus.rawValue, "filter": s.filter.rawValue, "rows": rows,
                "editing": editor != nil, "find": findMode != .none, "pathEdit": pathEditSide != nil,
                "summary": summary.stringValue, "status": s.status, "scrollY": Int(scroll.contentView.bounds.origin.y),
                "leftLines": m.left.lines.count, "rightLines": m.right.lines.count, "displayRows": s.displayCount,
                "canUndo": m.canUndo, "banner": !banner.isHidden,
                "anchors": m.anchors.map { [$0.l + 1, $0.r + 1] }, "alignPick": alignPick.map { "\($0.side.rawValue):\($0.line + 1)" } ?? NSNull(),
                "eol": [m.left.eolLabel, m.right.eolLabel], "whitespace": showWhitespace, "recovered": s.recovered,
            ] as [String: Any]
        }
        st["folder"] = session?.folder != nil ? folderPage.testState : NSNull()
        return st
    }

    // one `do:compare:` action; nil = done, else the error
    func testDo(_ a: String) -> String? {
        func side(_ w: String) -> CompareSide? { CompareSide(rawValue: w) }
        let parts = a.split(separator: ":", maxSplits: 2, omittingEmptySubsequences: false).map(String.init)
        if parts[0].hasPrefix("folder-") {
            return folderPage.testDo(String(parts[0].dropFirst(7)), arg: parts.count > 1 ? (parts.count > 2 ? parts[1] + ":" + parts[2] : parts[1]) : "")
        }
        switch parts[0] {
        case "next": jumpSection(1)
        case "prev": jumpSection(-1)
        case "copy-right": copyAcross(from: .left)
        case "copy-left": copyAcross(from: .right)
        case "swap": swapSides()
        case "reload": reload()
        case "undo": undo()
        case "redo": undo(redo: true)
        case "start": showStartPage()
        case "start-paste": startPasted()
        case "edit-begin": beginEdit()
        case "edit-commit": commitEditor()
        case "filter":
            guard parts.count > 1, let f = CompareFilter(rawValue: parts[1]) else { return "filter:all|diffs|same|context" }
            setFilter(f)
        case "save":
            guard parts.count > 1, let sd = side(parts[1]) else { return "save:left|right" }
            save(sd)
        case "select":
            guard parts.count > 1, let i = Int(parts[1]) else { return "select:N" }
            select(i)
        case "close-session":
            guard let i = selected else { return "no session" }
            closeSession(i, force: parts.count > 1 && parts[1] == "force")
        case "close-all":
            while !sessions.isEmpty { removeSession(0) }
        case "align":
            // align:L,R = left line L onto right line R (1-based, as shown)
            let n = (parts.count > 1 ? parts[1] : "").split(separator: ",").compactMap { Int($0) }
            guard n.count == 2 else { return "align:LEFT,RIGHT (1-based lines)" }
            alignPick = nil
            alignWith(side: .left, line: n[0] - 1)
            alignWith(side: .right, line: n[1] - 1)
        case "align-clear": clearAlignment(row: nil)
        case "trim":
            guard parts.count > 1, let sd = side(parts[1]) else { return "trim:left|right" }
            trimTrailing(sd)
        case "eol":
            let e: [String: EOL] = ["lf": .lf, "crlf": .crlf, "cr": .cr]
            guard parts.count > 2, let sd = side(parts[1]), let eol = e[parts[2]] else { return "eol:left|right:lf|crlf|cr" }
            convertEOL(sd, eol)
        case "whitespace":
            if (parts.count > 1 && parts[1] == "on") != showWhitespace { toggleWhitespace() }
        case "sheet-cancel":
            if let sh = window.attachedSheet { window.endSheet(sh, returnCode: .alertThirdButtonReturn) }
            confirmCard?.cancel()
            closeShortcutsCard()
        case "paste":
            // paste:left|right:TEXT (\n = newline)
            guard parts.count > 2, let sd = side(parts[1]) else { return "paste:left|right:TEXT" }
            setPasted(parts[2].replacingOccurrences(of: "\\n", with: "\n"), sd)
        case "edit":
            // edit:left|right:TEXT — the section editor on that side gets TEXT, commits
            guard parts.count > 2, let sd = side(parts[1]), let s = session else { return "edit:left|right:TEXT" }
            s.focus = sd
            beginEdit()
            editor?.string = parts[2].replacingOccurrences(of: "\\n", with: "\n")
            commitEditor()
        case "cursor":
            guard parts.count > 1, let n = Int(parts[1]) else { return "cursor:N" }
            moveCursor(n)
        case "key":
            guard parts.count > 1, let e = Self.keyEvent(parts.dropFirst().joined(separator: ":"), window: window) else {
                return "key:SPEC (ctrl+n, cmd+z, esc, return, tab, up, f5, a…)"
            }
            // the monitor's path when the window is key; else (a test run from a
            // terminal that keeps focus) straight to the view's own keys
            if window.isKeyWindow {
                if routeKey(e) != nil { window.firstResponder?.keyDown(with: e) }
            } else if !handleKey(e) {
                window.firstResponder?.keyDown(with: e)
            }
        default: return "unknown compare action \(parts[0])"
        }
        return nil
    }

    // "ctrl+shift+z", "esc", "f5", "a" -> a keyDown event
    static func keyEvent(_ spec: String, window: NSWindow) -> NSEvent? {
        let names: [String: (UInt16, String)] = [
            "esc": (53, "\u{1b}"), "return": (36, "\r"), "enter": (76, "\r"), "tab": (48, "\t"), "space": (49, " "),
            "up": (126, "\u{F700}"), "down": (125, "\u{F701}"), "left": (123, "\u{F702}"), "right": (124, "\u{F703}"),
            "pgup": (116, "\u{F72C}"), "pgdn": (121, "\u{F72D}"), "home": (115, "\u{F729}"), "end": (119, "\u{F72B}"),
            "f5": (96, "\u{F708}"), "delete": (51, "\u{7f}"), "/": (44, "/"),
        ]
        let letters: [Character: UInt16] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13,
            "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26,
            "8": 28, "0": 29, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
        ]
        var flags: NSEvent.ModifierFlags = []
        var key = ""
        for p in spec.lowercased().split(separator: "+").map(String.init) {
            switch p {
            case "cmd": flags.insert(.command)
            case "ctrl": flags.insert(.control)
            case "opt", "alt": flags.insert(.option)
            case "shift": flags.insert(.shift)
            default: key = p
            }
        }
        var code: UInt16
        var chars: String
        if let n = names[key] { (code, chars) = n }
        else if key.count == 1, let c = key.first, let k = letters[c] { code = k; chars = key }
        else { return nil }
        if flags.contains(.shift), chars.count == 1, chars.first!.isLetter { chars = chars.uppercased() }
        return NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                                windowNumber: window.windowNumber, context: nil, characters: chars,
                                charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)
    }
}

// the pasteboard as plain text (Copy Path, Copy Paths)
func copyText(_ s: String) {
    let pb = NSPasteboard.general
    pb.clearContents()
    pb.setString(s, forType: .string)
}

// MARK: - Ctrl+H/J/K/L panes (PaneNav.swift)
//
// sessions sidebar; start page: the two path boxes, Recent's filter and
// list; text compare: the two sides of the one pane view (focus = the
// side, like Tab / ← →); folder compare: FolderPage's own.
extension CompareWindow: PaneProvider {
    var navPanes: [NavPane] {
        var out: [NavPane] = []
        if let bar = pills, bar.vertical {
            out.append(NavPane("sidebar", bar, focus: { [weak bar] in bar?.takeKeyboardFocus() }))
        }
        if session == nil {
            var filter = NavPane.area("recent-filter", recentFilter)
            var recent = NavPane("recent", recentScroll, focus: { [weak self] in
                guard let self else { return }
                self.window.makeFirstResponder(self.recentList)
            })
            filter.normal = recent.focus
            recent.insert = filter.focus
            recent.vim = { [weak self] in self.map { .rows(CompareRecentVim($0)) } }
            out += [NavPane.area("left-path", leftBox), NavPane.area("right-path", rightBox), filter, recent]
        } else if session?.folder != nil {
            out += folderPage.navPanes
        } else {
            for side in [CompareSide.left, .right] {
                var p = NavPane(side.rawValue, scroll, part: { [weak self] in
                    guard let self else { return .zero }
                    let v = self.scroll.documentVisibleRect
                    let r = NSRect(x: self.pane.paneX(side), y: v.minY, width: self.pane.paneW, height: v.height)
                    return self.scroll.convert(r, from: self.pane)
                }, focus: { [weak self] in
                    guard let self, let s = self.session else { return }
                    _ = self.commitEditor()
                    s.focus = side
                    self.window.makeFirstResponder(self.pane)
                    self.syncAll()
                }, owns: { [weak self] r in
                    guard let self else { return false }
                    return NavPane.inside(r, self.scroll) && self.session?.focus == side
                })
                // normal mode walks the lines; i / a = the section editor
                p.vim = { [weak self] in self.map { .rows(CompareTextVim($0)) } }
                p.insert = { [weak self] in self?.beginEdit() }
                out.append(p)
            }
        }
        return out
    }
}

// vim normal mode (VimKeys.swift): the start page's Recent list, a text
// compare's lines on the focused side
final class CompareRecentVim: VimRows {
    private weak var w: CompareWindow?
    init(_ w: CompareWindow) { self.w = w }
    var vimCount: Int { w?.vimRecent.rows.count ?? 0 }
    var vimCursor: Int { w?.vimRecent.selection ?? 0 }
    var vimPage: Int {
        guard let l = w?.vimRecent else { return 10 }
        return max(1, Int(l.visibleRect.height / max(1, l.rowH)))
    }
    func vimText(_ row: Int) -> String {
        guard let r = w?.vimRecent.rows, r.indices.contains(row) else { return "" }
        return "\(r[row].left) \(r[row].right)"
    }
    func vimMove(to row: Int) { w?.vimMoveRecent(to: row) }
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])? {
        guard let l = w?.vimRecent else { return nil }
        return Self.shownRows(in: l, count: l.rows.count, rowH: l.rowH) {
            NSRect(x: 0, y: CGFloat($0) * l.rowH, width: l.bounds.width, height: l.rowH)
        }
    }
}

final class CompareTextVim: VimRows {
    private weak var w: CompareWindow?
    init(_ w: CompareWindow) { self.w = w }
    var vimCount: Int { w?.vimSession?.displayCount ?? 0 }
    var vimCursor: Int { w?.vimSession?.cursor ?? 0 }
    var vimPage: Int { w?.vimPageRows ?? 20 }
    func vimText(_ row: Int) -> String {
        guard let s = w?.vimSession, row >= 0, row < s.displayCount else { return "" }
        let line = s.model.rows[s.modelRow(row)].line(s.focus)
        return line >= 0 ? s.model.side(s.focus).lines[line] : ""
    }
    func vimMove(to row: Int) { w?.vimMoveCursor(to: row) }
    // the focused side's text column only (both sides share one view)
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])? {
        guard let w, let s = w.vimSession else { return nil }
        let p = w.vimPane
        return Self.shownRows(in: p, count: s.displayCount, rowH: p.rowH) { p.textRect(s.focus, row: $0) }
    }
}

extension CompareWindow {
    var vimRecent: CompareRecentList { recentList }
    var vimSession: CompareSession? { session }
    var vimPane: ComparePaneView { pane }
    var vimPageRows: Int { max(1, Int(scroll.contentView.bounds.height / max(1, pane.rowH))) }
    func vimMoveRecent(to row: Int) { moveRecent(row - recentList.selection) }
    func vimMoveCursor(to row: Int) {
        moveCursor(row)
        scrollCursorVisible()
    }
}
