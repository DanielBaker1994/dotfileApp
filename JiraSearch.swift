import AppKit

// MARK: - Jira live search (Cmd+F in the Jira window) + the value pickers
//
// JiraSearchPanel: a bar docked to the Jira window (child window, above it
// when there is room, else below). Free text (Jira `text ~`: title,
// description, comments) + Projects + "+ Filter" rows (assignee / reporter /
// status / type / priority / release / labels / dates / "… contains"). Every
// value Jira knows up front (users, releases, labels …) is a PICKER over the
// directory cache, never typed text. Controls are drawn in the Jira window's
// theme (JiraInputBox / JiraChoiceButton / ThemeButton). Return runs `jira_poll.py --live-search` (criteria JSON on stdin)
// which writes <outDir>/search.json — the Jira window's "search.json" tab —
// and the window jumps to that tab. Nothing is saved as a job; the last
// criteria are remembered (UserDefaults) so the panel reopens as you left it.
//
// JiraMultiPicker: a searchable multi-select over a FIXED list (projects /
// users / statuses … from ~/.cache/jira/directory.json, the weekly
// `directory` poll job). Typing only filters — a value that is not in the
// list cannot be entered, so a search never fails on a typo. Several picked
// values are ORed (`assignee in (…)`).
//
// Config ([jira] in commands.conf): search-date-ranges (default
// "today, 2d, 7d, 14d, 30d, 90d"), search-max-choices ("25, 50, 100, 250, 500").

// edit shortcuts for the jira windows' own key monitors (rule.md #1: the
// accessory app has no reliable Edit-menu key equivalents)
enum JiraEditKeys {
    static func route(_ e: NSEvent, in window: NSWindow) -> Bool {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        guard cmd || ctrl, let ed = window.firstResponder as? NSText else { return false }
        switch e.keyCode {
        case 9: ed.paste(nil)                                     // Cmd+V / Ctrl+V
        case 8: ed.copy(nil)                                      // Cmd+C / Ctrl+C
        case 0 where cmd: ed.selectAll(nil)                       // Cmd+A
        case 7 where cmd: ed.cut(nil)                             // Cmd+X
        case 6 where cmd:                                         // Cmd+Z / Cmd+Shift+Z
            if mods.contains(.shift) { ed.undoManager?.redo() } else { ed.undoManager?.undo() }
        default: return false
        }
        return true
    }
}

// ~/.cache/jira/directory.json (written by the `directory` poll job)
struct JiraDirectory {
    struct User { let id, name, username, email: String; let projects: [String] }
    struct Field { let id, name: String; let custom: Bool; let type: String }
    struct Version { let name, project, releaseDate: String; let released: Bool }
    var projects: [(key: String, name: String)] = []
    var users: [User] = []
    var statuses: [String] = []
    var issueTypes: [String] = []
    var priorities: [String] = []
    var fields: [Field] = []
    var versions: [Version] = []                          // unarchived releases per project
    var labels: [(name: String, projects: [String])] = []  // labels seen per project
    var fetchedAt = ""
    var isEmpty: Bool { fetchedAt.isEmpty }

    static func load() -> JiraDirectory {
        var d = JiraDirectory()
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: JiraPoll.directoryPath)),
              let o = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return d }
        d.fetchedAt = o["fetchedAt"] as? String ?? ""
        d.projects = (o["projects"] as? [[String: Any]] ?? []).compactMap { p in
            (p["key"] as? String).map { ($0, p["name"] as? String ?? "") }
        }
        d.users = (o["users"] as? [[String: Any]] ?? []).compactMap { u in
            guard let id = u["id"] as? String else { return nil }
            return User(id: id, name: u["name"] as? String ?? id, username: u["username"] as? String ?? "",
                        email: u["email"] as? String ?? "", projects: u["projects"] as? [String] ?? [])
        }
        d.statuses = o["statuses"] as? [String] ?? []
        d.issueTypes = o["issueTypes"] as? [String] ?? []
        d.priorities = o["priorities"] as? [String] ?? []
        d.fields = (o["fields"] as? [[String: Any]] ?? []).compactMap { f in
            guard let id = f["id"] as? String else { return nil }
            return Field(id: id, name: f["name"] as? String ?? id, custom: f["custom"] as? Bool ?? false,
                         type: f["type"] as? String ?? "")
        }
        d.versions = (o["versions"] as? [[String: Any]] ?? []).compactMap { v in
            guard let n = v["name"] as? String else { return nil }
            return Version(name: n, project: v["project"] as? String ?? "",
                           releaseDate: v["releaseDate"] as? String ?? "", released: v["released"] as? Bool ?? false)
        }
        d.labels = (o["labels"] as? [[String: Any]] ?? []).compactMap { l in
            (l["name"] as? String).map { ($0, l["projects"] as? [String] ?? []) }
        }
        return d
    }

    // directory projects + configured keys the directory doesn't know (yet)
    func projectOptions(extra: [String] = []) -> [JiraMultiPicker.Option] {
        var out = projects.map { JiraMultiPicker.Option(id: $0.key, title: $0.key, detail: $0.name) }
        for k in extra where !out.contains(where: { $0.id == k }) {
            out.append(.init(id: k, title: k, detail: ""))
        }
        return out
    }

    func userOptions(me: Bool = true) -> [JiraMultiPicker.Option] {
        var out: [JiraMultiPicker.Option] = me ? [.init(id: "currentUser()", title: "Me", detail: "currentUser()")] : []
        out += users.map { u in
            let who = [u.username == u.name ? "" : u.username, u.email].filter { !$0.isEmpty }
            let detail = (who + [u.projects.joined(separator: ", ")]).filter { !$0.isEmpty }.joined(separator: " · ")
            return .init(id: u.id, title: u.name, detail: detail)
        }
        return out
    }

    static func options(_ vals: [String]) -> [JiraMultiPicker.Option] {
        vals.map { .init(id: $0, title: $0, detail: "") }
    }

    // `in`: the picked projects (nil = all) — only their labels / releases
    private static func matches(_ ps: [String], _ scope: [String]?) -> Bool {
        guard let scope, !scope.isEmpty else { return true }
        return ps.contains(where: scope.contains)
    }

    func labelOptions(in scope: [String]?) -> [JiraMultiPicker.Option] {
        labels.filter { Self.matches($0.projects, scope) }
            .map { .init(id: $0.name, title: $0.name, detail: $0.projects.joined(separator: ", ")) }
    }

    // one option per release NAME (JQL matches fixVersion by name); the
    // detail lists every project that has it + its date / released state
    func versionOptions(in scope: [String]?) -> [JiraMultiPicker.Option] {
        var order: [String] = [], by: [String: [Version]] = [:]
        for v in versions where Self.matches([v.project], scope) {
            if by[v.name] == nil { order.append(v.name) }
            by[v.name, default: []].append(v)
        }
        return order.map { n in
            let vs = by[n] ?? []
            let date = vs.first { !$0.releaseDate.isEmpty }?.releaseDate ?? "no date"
            let state = vs.allSatisfy(\.released) ? "released" : "unreleased"
            return .init(id: n, title: n, detail: ([vs.map(\.project).joined(separator: ", "), date, state])
                            .joined(separator: " · "))
        }
    }
}

// MARK: - theme (the Cmd+F panel follows the Jira window's palette)

enum JiraTheme {
    // pickers in plain AppKit windows (Jira Config) follow the system look
    static var system: PopupColors {
        PopupColors(background: .windowBackgroundColor, border: .separatorColor, text: .labelColor,
                    dim: .secondaryLabelColor, highlight: .selectedContentBackgroundColor,
                    accent: .controlAccentColor)
    }
    static let height: CGFloat = 26
    static let radius: CGFloat = 6
    static let font = NSFont.systemFont(ofSize: 12)

    // an input's surface: faint fill, hairline; the accent ring when focused
    static func drawInput(_ bounds: NSRect, _ c: PopupColors, hover: Bool, focused: Bool) {
        let r = bounds.insetBy(dx: focused ? 1 : 0.5, dy: focused ? 1 : 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius)
        c.text.withAlphaComponent(hover ? 0.08 : 0.05).setFill()
        path.fill()
        path.lineWidth = focused ? 1.5 : 1
        (focused ? ButtonStyle.accent(c) : ButtonStyle.inputStroke(c)).setStroke()
        path.stroke()
    }

    static func baseline(_ f: NSFont = font, height h: CGFloat = height) -> CGFloat {
        ((h - (f.ascender - f.descender)) / 2 + f.ascender).rounded()
    }
}

// A themed single-line text input: the field sits borderless inside a
// drawn surface (padding + a focus ring in the theme's accent — no AppKit
// bezel / system focus ring).
final class JiraInputBox: NSView, PopupThemeable {
    let field = NSTextField()
    var colors = JiraTheme.system { didSet { restyle() } }
    private var focused = false { didSet { needsDisplay = true } }

    init(placeholder: String, font: NSFont = JiraTheme.font) {
        super.init(frame: .zero)
        field.isBordered = false
        field.isBezeled = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = font
        field.usesSingleLineMode = true
        field.cell?.wraps = false
        field.cell?.isScrollable = true
        field.placeholderString = placeholder
        field.translatesAutoresizingMaskIntoConstraints = false
        addSubview(field)
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            field.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            field.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: max(JiraTheme.height, ceil(font.ascender - font.descender) + 10)),
        ])
        let nc = NotificationCenter.default
        nc.addObserver(self, selector: #selector(began(_:)), name: NSControl.textDidBeginEditingNotification, object: field)
        nc.addObserver(self, selector: #selector(ended(_:)), name: NSControl.textDidEndEditingNotification, object: field)
        restyle()
    }
    required init?(coder: NSCoder) { fatalError() }
    deinit { NotificationCenter.default.removeObserver(self) }

    @objc private func began(_ n: Notification) { focused = true }
    @objc private func ended(_ n: Notification) { focused = false }

    func applyColors(_ c: PopupColors) { colors = c }

    private func restyle() {
        field.textColor = colors.text
        if let p = field.placeholderString, !p.isEmpty {
            field.placeholderAttributedString = NSAttributedString(string: p, attributes: [
                .foregroundColor: colors.dim.withAlphaComponent(0.8), .font: field.font ?? JiraTheme.font])
        }
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(field) }
    override func draw(_ dirty: NSRect) {
        JiraTheme.drawInput(bounds, colors, hover: false, focused: focused)
    }
}

// A themed pop-up / pull-down: a ButtonStyle surface with the chosen title
// and a chevron; click opens an NSMenu of `items`. `fixedTitle` = pull-down
// ("+ Filter"): the title never changes and the menu fires `onPick`.
final class JiraChoiceButton: NSView, PopupThemeable {
    var colors = JiraTheme.system { didSet { needsDisplay = true } }
    var items: [(title: String, value: String)] = [] { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var value: String? { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var fixedTitle: String?
    var prefix = ""                       // e.g. "Max " before the value
    var disabled: Set<String> = []        // menu items shown greyed out
    var onPick: ((String) -> Void)?
    private var hover = false, down = false
    private var trackingArea: NSTrackingArea?
    private let font = NSFont.systemFont(ofSize: 12, weight: .medium)

    override var isFlipped: Bool { true }
    func applyColors(_ c: PopupColors) { colors = c }

    var title: String {
        fixedTitle ?? (prefix + (items.first { $0.value == value }?.title ?? value ?? ""))
    }
    override var intrinsicContentSize: NSSize {
        let w = (title as NSString).size(withAttributes: [.font: font]).width
        return NSSize(width: ceil(w) + 34, height: JiraTheme.height)
    }
    override var firstBaselineOffsetFromTop: CGFloat { JiraTheme.baseline(font) }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }

    override func draw(_ dirty: NSRect) {
        let st: ButtonState = down ? .pressed : hover ? .hover : .idle
        ButtonStyle.draw(bounds, st, colors, radius: JiraTheme.radius)
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: ButtonStyle.text(.hover, colors)]
        let s = title as NSString
        let sz = s.size(withAttributes: attrs)
        s.draw(at: NSPoint(x: 10, y: (bounds.height - sz.height) / 2), withAttributes: attrs)
        ButtonStyle.chevron(in: NSRect(x: bounds.maxX - 20, y: 0, width: 14, height: bounds.height), color: colors.dim)
    }

    override func mouseDown(with event: NSEvent) {
        down = true
        needsDisplay = true
        let menu = NSMenu()
        menu.autoenablesItems = false
        var current: NSMenuItem?
        for it in items {
            let mi = NSMenuItem(title: it.title, action: #selector(picked(_:)), keyEquivalent: "")
            mi.target = self
            mi.representedObject = it.value
            mi.isEnabled = !disabled.contains(it.value)
            if fixedTitle == nil && it.value == value { mi.state = .on; current = mi }
            menu.addItem(mi)
        }
        menu.popUp(positioning: current, at: NSPoint(x: 0, y: current == nil ? bounds.height + 3 : 0), in: self)
        down = false
        needsDisplay = true
    }

    @objc private func picked(_ sender: NSMenuItem) {
        guard let v = sender.representedObject as? String else { return }
        if fixedTitle == nil { value = v }
        onPick?(v)
    }
}

// MARK: - JiraMultiPicker

final class JiraMultiPicker: NSView, NSTableViewDataSource, NSTableViewDelegate,
                             NSSearchFieldDelegate, NSPopoverDelegate, PopupThemeable {
    struct Option { let id: String; let title: String; let detail: String }
    private static let allID = "\u{0}all"

    var options: [Option] = [] { didSet { updateDisplay() } }
    private(set) var selected: [String] = []
    private(set) var isAll = false
    var allTitle: String?          // non-nil: offers an "All …" state
    var noun = "value"
    var onChange: (() -> Void)?
    var placeholder = "Choose…" { didSet { updateDisplay() } }
    // palette: the Jira window's theme in the Cmd+F panel, else the system's
    var colors = JiraTheme.system { didSet { needsDisplay = true } }
    func applyColors(_ c: PopupColors) { colors = c }

    private var popover: NSPopover?
    private let search = NSSearchField()
    private let table = NSTableView()
    private let countLabel = NSTextField(labelWithString: "")
    private var shown: [Option] = []
    private var monitor: Any?
    private var hover = false
    private var trackingArea: NSTrackingArea?
    private static let pillFont = NSFont.systemFont(ofSize: 11.5, weight: .medium)
    private static let chevronW: CGFloat = 22

    init(noun: String, allTitle: String? = nil) {
        self.noun = noun
        self.allTitle = allTitle
        super.init(frame: .zero)
        focusRingType = .none
        toolTip = "Choose \(noun)s (type to filter)"
        setContentHuggingPriority(.defaultLow, for: .horizontal)
        updateDisplay()
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: JiraTheme.height) }

    // set without firing onChange; unknown ids stay (listed as "not in the
    // directory") so an old value can still be seen and removed
    func set(_ ids: [String], all: Bool = false) {
        selected = all ? [] : ids
        isAll = all && allTitle != nil
        let missing = selected.filter { id in !options.contains { $0.id == id } }
        if !missing.isEmpty {
            options += missing.map { Option(id: $0, title: $0, detail: "not in the directory cache") }
        }
        updateDisplay()
    }

    private func title(of id: String) -> String { options.first { $0.id == id }?.title ?? id }

    private func updateDisplay() {
        let names = selected.map(title(of:))
        toolTip = isAll ? allTitle : names.isEmpty ? "Choose \(noun)s (type to filter)" : names.joined(separator: ", ")
        needsDisplay = true
    }

    // MARK: drawing — one surface, value pills inside, chevron in its own slot

    private var focused: Bool { window?.firstResponder === self || popover != nil }

    override func draw(_ dirty: NSRect) {
        let c = colors
        JiraTheme.drawInput(bounds, c, hover: hover, focused: focused)
        let chev = NSRect(x: bounds.maxX - Self.chevronW, y: 0, width: Self.chevronW - 6, height: bounds.height)
        ButtonStyle.chevron(in: chev, color: c.dim)
        let limit = chev.minX - 2
        let items = isAll ? [allTitle ?? ""] : selected.map(title(of:))
        if items.isEmpty {
            let p = options.isEmpty ? "(no \(noun)s cached)" : placeholder
            let attrs: [NSAttributedString.Key: Any] = [.font: JiraTheme.font,
                                                        .foregroundColor: c.dim.withAlphaComponent(0.8)]
            let sz = (p as NSString).size(withAttributes: attrs)
            (p as NSString).draw(with: NSRect(x: 9, y: (bounds.height - sz.height) / 2,
                                              width: max(0, limit - 9), height: sz.height),
                                 options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin], attributes: attrs)
            return
        }
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        let attrs: [NSAttributedString.Key: Any] = [.font: Self.pillFont, .foregroundColor: c.text,
                                                    .paragraphStyle: para]
        let pillH = bounds.height - 8
        func width(_ t: String) -> CGFloat { ceil((t as NSString).size(withAttributes: attrs).width) + 14 }
        var x: CGFloat = 4
        for (i, t) in items.enumerated() {
            let rest = items.count - i - 1
            let more = rest > 0 ? width("+\(rest)") + 4 : 0
            var w = min(width(t), 200)
            if x + w + more > limit {
                // no room: the first pill shrinks to fit, later ones fold into "+N"
                if i == 0 { w = max(24, limit - x - more) } else {
                    pill("+\(items.count - i)", x: x, w: width("+\(items.count - i)"), h: pillH, attrs: attrs, dim: true)
                    return
                }
            }
            pill(t, x: x, w: w, h: pillH, attrs: attrs, dim: false)
            x += w + 4
        }
    }

    private func pill(_ t: String, x: CGFloat, w: CGFloat, h: CGFloat,
                      attrs: [NSAttributedString.Key: Any], dim: Bool) {
        let r = NSRect(x: x, y: (bounds.height - h) / 2, width: w, height: h)
        colors.text.withAlphaComponent(dim ? 0.07 : 0.13).setFill()
        NSBezierPath(roundedRect: r, xRadius: 4, yRadius: 4).fill()
        var a = attrs
        if dim { a[.foregroundColor] = colors.dim }
        let th = (t as NSString).size(withAttributes: a).height
        (t as NSString).draw(with: NSRect(x: r.minX + 7, y: r.midY - th / 2, width: r.width - 14, height: th),
                             options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin], attributes: a)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }

    // align with form labels (NSGridView .firstBaseline rows)
    override var firstBaselineOffsetFromTop: CGFloat { JiraTheme.baseline() }
    override var lastBaselineOffsetFromBottom: CGFloat { JiraTheme.height - JiraTheme.baseline() }

    override func mouseDown(with event: NSEvent) { togglePopover(nil) }
    override var acceptsFirstResponder: Bool { true }
    override func becomeFirstResponder() -> Bool { needsDisplay = true; return true }
    override func resignFirstResponder() -> Bool { needsDisplay = true; return true }
    override func keyDown(with event: NSEvent) {
        // Space / Return / Down on the focused control open the list
        if [49, 36, 125].contains(event.keyCode) { togglePopover(nil) } else { super.keyDown(with: event) }
    }

    // MARK: popover

    @objc func togglePopover(_ sender: Any?) {
        if let p = popover { p.close(); return }
        if table.tableColumns.isEmpty {
            let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("o"))
            table.addTableColumn(col)
            table.headerView = nil
            table.rowHeight = 22
            table.dataSource = self
            table.delegate = self
            table.target = self
            table.action = #selector(rowClicked(_:))
            table.style = .plain
            table.intercellSpacing = NSSize(width: 0, height: 2)
            search.delegate = self
            search.sendsSearchStringImmediately = true
        }
        search.stringValue = ""
        search.placeholderString = "Filter \(noun)s — ↑↓ move · Return toggles · Esc closes"
        let sv = NSScrollView()
        sv.documentView = table
        sv.hasVerticalScroller = true
        sv.drawsBackground = false
        let clear = NSButton(title: "Clear", target: self, action: #selector(clearAll(_:)))
        let done = NSButton(title: "Done", target: self, action: #selector(togglePopover(_:)))
        for b in [clear, done] { b.bezelStyle = .rounded; b.controlSize = .small }
        countLabel.font = .systemFont(ofSize: 11)
        countLabel.textColor = .secondaryLabelColor
        let footer = NSStackView(views: [countLabel, NSView(), clear, done])
        footer.orientation = .horizontal
        let stack = NSStackView(views: [search, sv, footer])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 10, bottom: 8, right: 10)
        for v in [search, sv, footer] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            v.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -20).isActive = true
        }
        let w = max(340, bounds.width)
        // list height fits the options (4 … 12 rows visible)
        let listH = CGFloat(min(12, max(4, options.count + (allTitle == nil ? 0 : 1)))) * 24 + 4
        let h = listH + 80
        stack.frame = NSRect(x: 0, y: 0, width: w, height: h)
        sv.heightAnchor.constraint(equalToConstant: listH).isActive = true
        let vc = NSViewController()
        vc.view = stack
        let p = NSPopover()
        p.behavior = .transient
        p.contentViewController = vc
        p.contentSize = NSSize(width: w, height: h)
        p.delegate = self
        popover = p
        needsDisplay = true
        refilter()
        p.show(relativeTo: bounds, of: self, preferredEdge: .maxY)
        search.window?.makeFirstResponder(search)
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, let win = self.search.window, win.isKeyWindow else { return e }
            return JiraEditKeys.route(e, in: win) ? nil : e
        }
    }

    func popoverDidClose(_ notification: Notification) {
        popover = nil
        needsDisplay = true
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        window?.makeFirstResponder(self)
    }

    private func refilter() {
        let q = search.stringValue.lowercased().split(separator: " ").map(String.init)
        var list: [Option]
        if q.isEmpty {
            // picked first (in pick order), then the rest in list order
            let picked = selected.compactMap { id in options.first { $0.id == id } }
            list = picked + options.filter { !selected.contains($0.id) }
            if let a = allTitle { list.insert(Option(id: Self.allID, title: a, detail: ""), at: 0) }
        } else {
            list = options.filter { o in
                let hay = "\(o.title) \(o.detail) \(o.id)".lowercased()
                return q.allSatisfy { hay.contains($0) }
            }
        }
        shown = list
        table.reloadData()
        if !shown.isEmpty { table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false) }
        updateCount()
    }

    private func updateCount() {
        countLabel.stringValue = isAll ? (allTitle ?? "") :
            "\(selected.count) selected · \(shown.count) shown" + (selected.count > 1 ? " (ORed)" : "")
    }

    private func isOn(_ o: Option) -> Bool { o.id == Self.allID ? isAll : selected.contains(o.id) }

    private func toggle(_ row: Int) {
        guard shown.indices.contains(row) else { return }
        let o = shown[row]
        if o.id == Self.allID {
            isAll.toggle()
            if isAll { selected = [] }
        } else if let i = selected.firstIndex(of: o.id) {
            selected.remove(at: i)
        } else {
            selected.append(o.id)
            isAll = false
        }
        table.reloadData()
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        updateDisplay()
        updateCount()
        onChange?()
    }

    @objc private func rowClicked(_ sender: Any?) { toggle(table.clickedRow) }

    @objc private func clearAll(_ sender: Any?) {
        selected = []
        isAll = false
        refilter()
        updateDisplay()
        onChange?()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { shown.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard shown.indices.contains(row) else { return nil }
        let o = shown[row]
        let img = NSImageView(image: NSImage(systemSymbolName: isOn(o) ? "checkmark.square.fill" : "square",
                                             accessibilityDescription: nil) ?? NSImage())
        img.contentTintColor = isOn(o) ? ButtonStyle.accent(colors) : .tertiaryLabelColor
        let t = NSTextField(labelWithString: o.title)
        t.lineBreakMode = .byTruncatingTail
        if o.id == Self.allID { t.font = .systemFont(ofSize: 13, weight: .semibold) }
        let d = NSTextField(labelWithString: o.detail)
        d.textColor = .secondaryLabelColor
        d.font = .systemFont(ofSize: 11)
        d.lineBreakMode = .byTruncatingTail
        d.setContentCompressionResistancePriority(.defaultLow - 10, for: .horizontal)
        let s = NSStackView(views: [img, t, d])
        s.spacing = 6
        let c = NSTableCellView()
        s.translatesAutoresizingMaskIntoConstraints = false
        c.addSubview(s)
        NSLayoutConstraint.activate([
            s.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 4),
            s.trailingAnchor.constraint(lessThanOrEqualTo: c.trailingAnchor, constant: -4),
            s.centerYAnchor.constraint(equalTo: c.centerYAnchor),
        ])
        return c
    }

    func controlTextDidChange(_ obj: Notification) { refilter() }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        let r = table.selectedRow
        switch sel {
        case #selector(NSResponder.moveDown(_:)), #selector(NSResponder.insertTab(_:)):
            let n = min(shown.count - 1, r + 1)
            if n >= 0 { table.selectRowIndexes(IndexSet(integer: n), byExtendingSelection: false); table.scrollRowToVisible(n) }
            return true
        case #selector(NSResponder.moveUp(_:)), #selector(NSResponder.insertBacktab(_:)):
            let n = max(0, r - 1)
            if !shown.isEmpty { table.selectRowIndexes(IndexSet(integer: n), byExtendingSelection: false); table.scrollRowToVisible(n) }
            return true
        case #selector(NSResponder.insertNewline(_:)):
            toggle(r >= 0 ? r : 0)
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            if search.stringValue.isEmpty { popover?.close() } else { search.stringValue = ""; refilter() }
            return true
        default:
            return false
        }
    }
}

// MARK: - JiraSearchPanel

private final class JiraKeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    // text selection + caret in the theme's colors (not the Mac's accent)
    var selectionAttributes: [NSAttributedString.Key: Any]?
    var caretColor: NSColor?
    override func fieldEditor(_ createFlag: Bool, for object: Any?) -> NSText? {
        let ed = super.fieldEditor(createFlag, for: object)
        if let tv = ed as? NSTextView {
            if let a = selectionAttributes { tv.selectedTextAttributes = a }
            if let c = caretColor { tv.insertionPointColor = c }
        }
        return ed
    }
}

final class JiraSearchPanel: NSObject, NSTextFieldDelegate {
    private static var live: JiraSearchPanel?
    private static var parkedAt: Date?     // detached by a window rebuild (reattach soon after)
    private static let stateKey = "jiraLiveSearch.state"

    static func toggle(on w: PopupWindow, controller: SwitcherController) {
        if let p = live, p.panel.isVisible, p.host === w { p.close(); return }
        let p = live ?? JiraSearchPanel(controller: controller)
        live = p
        p.attach(to: w)
    }

    // the jira window is hiding / being rebuilt: park the panel
    static func detach(from w: PopupWindow) {
        guard let p = live, p.host === w, p.panel.isVisible else { return }
        parkedAt = Date()
        p.unhook()
    }

    // the rebuilt jira window: bring a panel parked moments ago back
    static func reattach(to w: PopupWindow) {
        guard let p = live, let t = parkedAt, Date().timeIntervalSince(t) < 3 else { return }
        parkedAt = nil
        p.attach(to: w)
    }

    // criterion kinds for "+ Filter". Everything with a known set of values
    // is a picker over the directory cache (never free text); only the
    // "… contains" rows take typed text.
    private enum ValueKind { case users, list, date, text }
    private struct Kind { let key: String; let title: String; let value: ValueKind }
    private final class Row {
        let kind: Kind
        let view: NSStackView
        let control: NSView      // JiraMultiPicker / JiraChoiceButton / NSTextField
        init(kind: Kind, view: NSStackView, control: NSView) { self.kind = kind; self.view = view; self.control = control }
    }

    private weak var controller: SwitcherController?
    fileprivate weak var host: PopupWindow?
    let panel: NSPanel
    private var monitor: Any?
    private var dir = JiraDirectory()
    private var info: [String: Any] = [:]
    private var rows: [Row] = []
    private var lastJQL = ""
    private var lastCurl = ""
    private var restored = false
    private var searching = false
    private var colors = PopupColors()

    private let fx = NSVisualEffectView()
    private let tint = NSView()
    private let textBox = JiraInputBox(placeholder: "Search text — matches title, description and comments · Return searches",
                                       font: .systemFont(ofSize: 14))
    private var textField: NSTextField { textBox.field }
    private let projects = JiraMultiPicker(noun: "project", allTitle: "All projects")
    private let addFilter = JiraChoiceButton()
    private let searchButton: ThemeButton
    private let rowsStack = NSStackView()
    // filter rows scroll inside a capped area so the panel stays docked
    // above the Jira window however many rows there are
    private let rowsScroll = NSScrollView()
    private let rowsDoc = JiraFlippedView()
    private var rowsHeight: NSLayoutConstraint?
    private let status = NSTextField(labelWithString: "")
    private let spinner = NSProgressIndicator()
    private let maxChoice = JiraChoiceButton()
    private let fetchButton: ThemeButton
    private let stack = NSStackView()
    private var labels: [NSTextField] = []    // secondary text (row titles): themed dim

    // one font / radius for every button in the panel
    private static var buttonConfig: PopupConfig {
        var c = PopupConfig(name: "jira-search")
        c.buttonFontSize = 12
        c.buttonRadius = JiraTheme.radius
        return c
    }

    private static var dateRanges: [String] {
        let v = jiraConfigValue("search-date-ranges") ?? ""
        let r = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        return r.isEmpty ? ["today", "2d", "7d", "14d", "30d", "90d"] : r
    }
    private static var maxChoices: [String] {
        let v = jiraConfigValue("search-max-choices") ?? ""
        let r = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { Int($0) != nil }
        return r.isEmpty ? ["25", "50", "100", "250", "500"] : r
    }
    private static func dateTitle(_ r: String) -> String {
        if r == "today" { return "Today" }
        guard let n = Int(r.dropLast()), let u = r.last else { return r }
        let unit = ["h": "hour", "d": "day", "w": "week"][String(u)] ?? String(u)
        return "Last \(n) \(unit)\(n == 1 ? "" : "s")"
    }

    private static func themeButton(_ title: String, symbol: String? = nil, flat: Bool = false,
                                     tip: String, _ action: @escaping () -> Void) -> ThemeButton {
        let b = ThemeButton(config: buttonConfig, title: title, symbol: symbol)
        b.flat = flat
        b.onClick = action
        b.toolTip = tip
        let tw = ceil((title as NSString).size(withAttributes: [.font: ButtonStyle.font(12, .on)]).width)
        let w = title.isEmpty ? JiraTheme.height : tw + (symbol == nil ? 24 : 40)
        b.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([b.widthAnchor.constraint(equalToConstant: w),
                                     b.heightAnchor.constraint(equalToConstant: JiraTheme.height)])
        return b
    }

    private init(controller: SwitcherController) {
        self.controller = controller
        panel = JiraKeyPanel(contentRect: NSRect(x: 0, y: 0, width: 760, height: 90),
                             styleMask: [.borderless], backing: .buffered, defer: false)
        var run: (() -> Void)?, fetch: (() -> Void)?
        searchButton = Self.themeButton("Search", symbol: "magnifyingglass", tip: "Search (Return / ⌘Return)") { run?() }
        fetchButton = Self.themeButton("Fetch from Jira", symbol: "arrow.down.circle",
                                       tip: "Run the weekly directory job now: projects, users, statuses, releases, labels") { fetch?() }
        super.init()
        run = { [weak self] in self?.run(nil) }
        fetch = { [weak self] in self?.fetchDirectory(nil) }
        panel.isReleasedWhenClosed = false
        panel.hasShadow = true
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hidesOnDeactivate = false
        panel.autorecalculatesKeyViewLoop = true
        build()
    }

    private func build() {
        // surface = the Jira window's own: its blur material + its card tint
        fx.state = .active
        fx.blendingMode = .behindWindow
        fx.wantsLayer = true
        fx.layer?.cornerRadius = 10
        fx.layer?.masksToBounds = true
        panel.contentView = fx
        tint.wantsLayer = true
        tint.layer?.cornerRadius = 10
        tint.layer?.borderWidth = 1
        tint.frame = fx.bounds
        tint.autoresizingMask = [.width, .height]
        fx.addSubview(tint)

        textField.delegate = self
        textField.target = self
        textField.action = #selector(run(_:))
        textBox.setContentHuggingPriority(.defaultLow - 1, for: .horizontal)
        projects.placeholder = "Projects…"
        projects.onChange = { [weak self] in self?.projectsChanged() }
        projects.widthAnchor.constraint(equalToConstant: 230).isActive = true
        addFilter.fixedTitle = "+ Filter"
        addFilter.onPick = { [weak self] key in self?.addFilterPicked(key) }
        addFilter.toolTip = "Add a criterion (all criteria are ANDed)"
        let close = Self.themeButton("", symbol: "xmark", flat: true, tip: "Close (Esc / ⌘F)") { [weak self] in self?.close() }
        let top = hrow([textBox, projects, addFilter, searchButton, close])

        rowsStack.orientation = .vertical
        rowsStack.alignment = .leading
        rowsStack.spacing = 6
        rowsScroll.drawsBackground = false
        rowsScroll.borderType = .noBorder
        rowsScroll.hasVerticalScroller = true
        rowsScroll.autohidesScrollers = true
        rowsScroll.scrollerStyle = .overlay
        rowsScroll.documentView = rowsDoc
        rowsScroll.isHidden = true
        rowsDoc.translatesAutoresizingMaskIntoConstraints = false
        rowsStack.translatesAutoresizingMaskIntoConstraints = false
        rowsDoc.addSubview(rowsStack)
        let clip = rowsScroll.contentView
        let rh = rowsScroll.heightAnchor.constraint(equalToConstant: 0)
        rowsHeight = rh
        NSLayoutConstraint.activate([
            rh,
            rowsDoc.leadingAnchor.constraint(equalTo: clip.leadingAnchor),
            rowsDoc.topAnchor.constraint(equalTo: clip.topAnchor),
            rowsDoc.widthAnchor.constraint(equalTo: clip.widthAnchor),
            rowsStack.leadingAnchor.constraint(equalTo: rowsDoc.leadingAnchor),
            rowsStack.trailingAnchor.constraint(equalTo: rowsDoc.trailingAnchor),
            rowsStack.topAnchor.constraint(equalTo: rowsDoc.topAnchor),
            rowsStack.bottomAnchor.constraint(equalTo: rowsDoc.bottomAnchor),
        ])

        status.font = .systemFont(ofSize: 11)
        status.lineBreakMode = .byTruncatingTail
        status.isSelectable = false
        status.setContentCompressionResistancePriority(.defaultLow - 10, for: .horizontal)
        status.setContentHuggingPriority(.defaultLow - 1, for: .horizontal)
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        maxChoice.prefix = "Max "
        maxChoice.items = Self.maxChoices.map { ($0, $0) }
        maxChoice.onPick = { [weak self] v in self?.maxChanged(v) }
        maxChoice.toolTip = "Max results per search (liveSearch.maxResults in config.json)"
        let copyJQL = Self.themeButton("Copy JQL", tip: "Copy the last search's JQL") { [weak self] in
            self.map { $0.copy($0.lastJQL, "JQL") }
        }
        let copyCurl = Self.themeButton("Copy curl", tip: "Copy the last search's request (includes the token)") { [weak self] in
            self.map { $0.copy($0.lastCurl, "curl (includes the token)") }
        }
        let footer = hrow([spinner, status, fetchButton, maxChoice, copyJQL, copyCurl])

        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
        for v in [top, rowsScroll, footer] as [NSView] { stack.addArrangedSubview(v) }
        stack.translatesAutoresizingMaskIntoConstraints = false
        fx.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: fx.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: fx.trailingAnchor),
            stack.topAnchor.constraint(equalTo: fx.topAnchor),
        ])
        for v in [top, rowsScroll, footer] as [NSView] {
            v.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -24).isActive = true
        }
    }

    private func hrow(_ views: [NSView]) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .horizontal
        s.spacing = 8
        s.alignment = .centerY
        return s
    }

    // the Jira window's palette on every surface, control and label
    private func applyTheme(_ cfg: PopupConfig) {
        colors = cfg.colors
        let light = ButtonStyle.luminance(colors.background) > 0.45
        panel.appearance = NSAppearance(named: light ? .aqua : .darkAqua)
        fx.material = cfg.material
        tint.layer?.backgroundColor = colors.background.withAlphaComponent(max(cfg.tintAlpha, 0.9)).cgColor
        tint.layer?.borderColor = colors.border.withAlphaComponent(0.25).cgColor
        let jp = panel as? JiraKeyPanel
        jp?.selectionAttributes = ButtonStyle.selection(colors)
        jp?.caretColor = colors.text
        func walk(_ v: NSView) {
            (v as? PopupThemeable)?.applyColors(colors)
            v.subviews.forEach(walk)
        }
        walk(fx)
        for l in labels { l.textColor = colors.dim }
        status.textColor = colors.dim
    }

    // MARK: attach / detach

    private func attach(to w: PopupWindow) {
        if let old = host, old !== w { old.nativeWindow.removeChildWindow(panel) }
        host = w
        applyTheme(w.config)
        reloadLists()
        panel.level = w.nativeWindow.level
        place()
        w.nativeWindow.addChildWindow(panel, ordered: .above)
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(textField)
        installKeys()
        loadInfo()
    }

    private func unhook() {
        host?.nativeWindow.removeChildWindow(panel)
        panel.orderOut(nil)
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
    }

    private func close() {
        saveState()
        let h = host
        unhook()
        h?.nativeWindow.makeKeyAndOrderFront(nil)
    }

    // docked above the jira window; when the filter rows outgrow the room
    // above, THEY scroll (the panel never jumps in front of the window).
    // Only when not even one row fits above: below, else inside its top.
    private func place() {
        guard let hw = host?.nativeWindow else { return }
        rowsScroll.isHidden = rows.isEmpty
        rowsDoc.layoutSubtreeIfNeeded()
        let natural = rows.isEmpty ? 0 : ceil(rowsStack.fittingSize.height)
        rowsHeight?.constant = natural
        stack.layoutSubtreeIfNeeded()
        let full = ceil(stack.fittingSize.height)
        let chrome = full - natural          // search row + footer + insets
        let minRows = min(natural, 34)       // at least one filter row visible
        let f = hw.frame
        let width = max(f.width, 680)
        let vis = (hw.screen ?? NSScreen.main)?.visibleFrame ?? f
        let above = vis.maxY - (f.maxY + 6)
        let below = (f.minY - 6) - vis.minY
        var y: CGFloat, rowsH = natural
        if above >= chrome + minRows {
            rowsH = min(natural, above - chrome)
            y = f.maxY + 6
        } else if below >= chrome + minRows {
            rowsH = min(natural, below - chrome)
            y = f.minY - 6 - (chrome + rowsH)
        } else {
            rowsH = min(natural, max(minRows, f.height * 0.5 - chrome))
            y = f.maxY - (chrome + rowsH) - 44
        }
        rowsHeight?.constant = rowsH
        let h = chrome + rowsH
        panel.setFrame(NSRect(x: f.minX, y: y, width: width, height: h), display: true)
    }

    // keep the newest filter row in view when the rows scroll
    private func scrollRowsToBottom() {
        rowsDoc.layoutSubtreeIfNeeded()
        let clip = rowsScroll.contentView
        clip.scroll(to: NSPoint(x: 0, y: max(0, rowsDoc.frame.height - clip.bounds.height)))
        rowsScroll.reflectScrolledClipView(clip)
    }

    private func installKeys() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.panel.isKeyWindow else { return e }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            if e.keyCode == 53 { self.close(); return nil }                             // Esc
            if mods.contains(.command) && e.keyCode == 3 { self.close(); return nil }   // Cmd+F
            if mods.contains(.command) && e.keyCode == 36 { self.run(nil); return nil } // Cmd+Return
            return JiraEditKeys.route(e, in: self.panel) ? nil : e
        }
    }

    // MARK: data

    private func reloadLists() {
        dir = JiraDirectory.load()
        projects.options = dir.projectOptions(extra: info["projectKeys"] as? [String] ?? [])
        for r in rows { fillOptions(r) }
        fetchButton.isHidden = !dir.isEmpty && !dir.versions.isEmpty
        if dir.isEmpty && status.stringValue.isEmpty {
            setStatus("Nothing cached yet — the pickers fill after the weekly directory job runs "
                      + "(or Fetch from Jira now).", .secondaryLabelColor)
        }
        rebuildFilterMenu()
        if !restored {
            restored = true
            restoreState()
        }
    }

    // describe: team project keys, the field catalog (labels, + Filter), max results
    private func loadInfo() {
        JiraPoll.run("jira_poll.py", ["--describe"]) { [weak self] _, out, _ in
            guard let self, let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] else { return }
            self.info = d
            self.projects.options = self.dir.projectOptions(extra: d["projectKeys"] as? [String] ?? [])
            // first use: the team's projects (team.json project_keys), else all
            if self.projects.selected.isEmpty && !self.projects.isAll && !self.hasSavedState {
                let pk = d["projectKeys"] as? [String] ?? []
                self.projects.set(pk, all: pk.isEmpty)
            }
            if self.maxChoice.value == nil, let m = (d["liveSearch"] as? [String: Any])?["maxResults"] as? Int {
                if !self.maxChoice.items.contains(where: { $0.value == String(m) }) {
                    self.maxChoice.items.append((String(m), String(m)))
                }
                self.maxChoice.value = String(m)
            }
            self.rebuildFilterMenu()
            self.relabelRows()
        }
    }

    // a field's one label (Jira Config ▸ Definitions ▸ Fields), else `fallback`
    private func fieldLabel(_ field: String, _ fallback: String) -> String {
        for c in info["catalog"] as? [[String: Any]] ?? [] where c["field"] as? String == field {
            if let l = c["label"] as? String, !l.isEmpty { return l }
        }
        return fallback
    }

    private var kinds: [Kind] {
        var k: [Kind] = [
            Kind(key: "assignee", title: fieldLabel("assignee", "Assignee"), value: .users),
            Kind(key: "reporter", title: fieldLabel("reporter", "Reporter"), value: .users),
            Kind(key: "status", title: fieldLabel("status", "Status"), value: .list),
            Kind(key: "issuetype", title: "Issue type", value: .list),
            Kind(key: "priority", title: fieldLabel("priority", "Priority"), value: .list),
            Kind(key: "fixVersion", title: fieldLabel("release", "Release"), value: .list),
            Kind(key: "labels", title: fieldLabel("labels", "Labels"), value: .list),
            Kind(key: "updated", title: "\(fieldLabel("updated", "Updated")) within", value: .date),
            Kind(key: "created", title: "Created within", value: .date),
            Kind(key: "resolved", title: "Resolved within", value: .date),
            Kind(key: "field:title", title: "\(fieldLabel("title", "Summary")) contains", value: .text),
            Kind(key: "field:description", title: "\(fieldLabel("description", "Description")) contains", value: .text),
        ]
        // every other known text column (team custom fields, raw ids) as "contains"
        let skip: Set<String> = ["key", "title", "status", "assignee", "release", "releaseLabel",
                                 "releaseDate", "releaseStatus", "priority", "labels", "description",
                                 "reporter", "project", "updated", "comments"]
        for c in info["catalog"] as? [[String: Any]] ?? [] {
            guard let f = c["field"] as? String, !skip.contains(f) else { continue }
            let label = (c["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? f
            k.append(Kind(key: "field:\(f)", title: "\(label) contains", value: .text))
        }
        return k
    }

    private func rebuildFilterMenu() {
        let ks = kinds
        addFilter.items = ks.map { ($0.title, $0.key) }
        addFilter.disabled = Set(rows.map(\.kind.key))
    }

    // the catalog arrived after rows were restored: show the fields' labels
    private func relabelRows() {
        let ks = kinds
        for r in rows {
            guard let t = ks.first(where: { $0.key == r.kind.key })?.title,
                  let l = r.view.arrangedSubviews.first as? NSTextField else { continue }
            l.stringValue = t
        }
    }

    // MARK: criterion rows

    private func addFilterPicked(_ key: String) {
        guard let k = kinds.first(where: { $0.key == key }), !rows.contains(where: { $0.kind.key == key }) else { return }
        let r = addRow(k)
        if let p = r.control as? JiraMultiPicker {
            DispatchQueue.main.async { p.togglePopover(nil) }
        } else if r.control is NSTextField {
            panel.makeFirstResponder(r.control)
        }
        saveState()
    }

    @discardableResult
    private func addRow(_ k: Kind) -> Row {
        let label = NSTextField(labelWithString: k.title)
        label.alignment = .right
        label.font = .systemFont(ofSize: 12)
        label.textColor = colors.dim
        label.lineBreakMode = .byTruncatingTail
        label.widthAnchor.constraint(equalToConstant: 150).isActive = true
        labels.append(label)
        let control: NSView, shown: NSView
        switch k.value {
        case .users, .list:
            let p = JiraMultiPicker(noun: k.value == .users ? "user" : k.title.lowercased())
            p.onChange = { [weak self] in self?.saveState() }
            p.colors = colors
            control = p
            shown = p
        case .date:
            let p = JiraChoiceButton()
            p.items = Self.dateRanges.map { (Self.dateTitle($0), $0) }
            p.value = Self.dateRanges[min(2, Self.dateRanges.count - 1)]
            p.onPick = { [weak self] _ in self?.saveState() }
            p.colors = colors
            control = p
            shown = p
        case .text:
            let box = JiraInputBox(placeholder: "text — Return searches")
            box.colors = colors
            box.field.target = self
            box.field.action = #selector(run(_:))
            box.field.delegate = self
            control = box.field
            shown = box
        }
        shown.setContentHuggingPriority(.defaultLow - 1, for: .horizontal)
        let remove = Self.themeButton("", symbol: "minus.circle", flat: true, tip: "Remove this filter") { [weak self] in
            self?.removeRow(k.key)
        }
        (remove as PopupThemeable).applyColors(colors)
        let v = hrow([label, shown, remove])
        let r = Row(kind: k, view: v, control: control)
        rows.append(r)
        fillOptions(r)
        rowsStack.addArrangedSubview(v)
        v.widthAnchor.constraint(equalTo: rowsStack.widthAnchor).isActive = true
        rebuildFilterMenu()
        if panel.isVisible { place(); scrollRowsToBottom() }
        return r
    }

    // picked projects scope the label / release lists (nil = all projects)
    private var projectScope: [String]? {
        projects.isAll || projects.selected.isEmpty ? nil : projects.selected
    }

    private func fillOptions(_ r: Row) {
        guard let p = r.control as? JiraMultiPicker else { return }
        let keep = p.selected
        switch r.kind.key {
        case "assignee", "reporter": p.options = dir.userOptions()
        case "status": p.options = JiraDirectory.options(dir.statuses)
        case "issuetype": p.options = JiraDirectory.options(dir.issueTypes)
        case "priority": p.options = JiraDirectory.options(dir.priorities)
        case "labels": p.options = dir.labelOptions(in: projectScope)
        case "fixVersion": p.options = dir.versionOptions(in: projectScope)
        default: break
        }
        p.set(keep)
    }

    private func projectsChanged() {
        for r in rows where ["labels", "fixVersion"].contains(r.kind.key) { fillOptions(r) }
        saveState()
    }

    private func removeRow(_ key: String) {
        guard let i = rows.firstIndex(where: { $0.kind.key == key }) else { return }
        if let l = rows[i].view.arrangedSubviews.first as? NSTextField { labels.removeAll { $0 === l } }
        rows[i].view.removeFromSuperview()
        rows.remove(at: i)
        rebuildFilterMenu()
        place()
        saveState()
    }

    func controlTextDidEndEditing(_ obj: Notification) { saveState() }

    // MARK: criteria / state

    private func criteria() -> [String: Any] {
        var c: [String: Any] = [:]
        let t = textField.stringValue.trimmingCharacters(in: .whitespaces)
        if !t.isEmpty { c["text"] = t }
        if !projects.isAll && !projects.selected.isEmpty { c["projects"] = projects.selected }
        var fields: [String: String] = [:]
        for r in rows {
            switch r.control {
            case let p as JiraMultiPicker where !p.selected.isEmpty:
                c[r.kind.key] = p.selected
            case let p as JiraChoiceButton:
                c[r.kind.key] = p.value ?? ""
            case let f as NSTextField:
                let v = f.stringValue.trimmingCharacters(in: .whitespaces)
                guard !v.isEmpty, r.kind.key.hasPrefix("field:") else { break }
                fields[String(r.kind.key.dropFirst(6))] = v
            default: break
            }
        }
        if !fields.isEmpty { c["fields"] = fields }
        if let m = Int(maxChoice.value ?? ""), m > 0 { c["maxResults"] = m }
        return c
    }

    private var hasSavedState: Bool { UserDefaults.standard.data(forKey: Self.stateKey) != nil }

    private func saveState() {
        var rs: [[String: Any]] = []
        for r in rows {
            var o: [String: Any] = ["key": r.kind.key]
            switch r.control {
            case let p as JiraMultiPicker: o["values"] = p.selected
            case let p as JiraChoiceButton: o["value"] = p.value ?? ""
            case let f as NSTextField: o["value"] = f.stringValue
            default: break
            }
            rs.append(o)
        }
        let st: [String: Any] = ["text": textField.stringValue, "projects": projects.selected,
                                 "projectsAll": projects.isAll, "rows": rs]
        if let d = try? JSONSerialization.data(withJSONObject: st) {
            UserDefaults.standard.set(d, forKey: Self.stateKey)
        }
    }

    private func restoreState() {
        guard let d = UserDefaults.standard.data(forKey: Self.stateKey),
              let st = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] else { return }
        textField.stringValue = st["text"] as? String ?? ""
        projects.set(st["projects"] as? [String] ?? [], all: st["projectsAll"] as? Bool ?? false)
        for o in st["rows"] as? [[String: Any]] ?? [] {
            guard let key = o["key"] as? String else { continue }
            // a column row needs the catalog (not loaded yet): rebuild its kind;
            // retired kinds (Raw JQL) are dropped
            let k = kinds.first { $0.key == key }
                ?? (key.hasPrefix("field:") ? Kind(key: key, title: "\(key.dropFirst(6)) contains", value: .text) : nil)
            guard let k, !rows.contains(where: { $0.kind.key == key }) else { continue }
            let r = addRow(k)
            switch r.control {
            case let p as JiraMultiPicker:
                // labels / releases used to be typed "a, b" text
                let old = (o["value"] as? String ?? "").split(separator: ",")
                    .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                p.set(o["values"] as? [String] ?? old)
            case let p as JiraChoiceButton:
                if let v = o["value"] as? String, p.items.contains(where: { $0.value == v }) { p.value = v }
            case let f as NSTextField: f.stringValue = o["value"] as? String ?? ""
            default: break
            }
        }
    }

    // MARK: actions

    // .secondaryLabelColor / .labelColor map onto the theme's dim / text
    private func setStatus(_ s: String, _ c: NSColor) {
        status.stringValue = s
        status.textColor = c == .secondaryLabelColor ? colors.dim : c == .labelColor ? colors.text : c
        status.toolTip = s
    }

    @objc private func run(_ sender: Any?) {
        guard !searching else { return }
        let crit = criteria()
        saveState()
        guard let data = try? JSONSerialization.data(withJSONObject: crit) else { return }
        searching = true
        spinner.startAnimation(nil)
        searchButton.alphaValue = 0.5
        setStatus("Searching…", .secondaryLabelColor)
        JiraPoll.run("jira_poll.py", ["--live-search"], stdin: String(decoding: data, as: UTF8.self)) { [weak self] code, out, err in
            guard let self else { return }
            self.searching = false
            self.spinner.stopAnimation(nil)
            self.searchButton.alphaValue = 1
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            self.lastJQL = r["jql"] as? String ?? self.lastJQL
            self.lastCurl = r["curl"] as? String ?? self.lastCurl
            guard r["ok"] as? Bool == true else {
                let e = r["error"] as? String ?? JiraPoll.errorLine(err, fallback: "search failed (exit \(code))")
                self.setStatus("✗ \(e)", .systemRed)
                return
            }
            let n = r["count"] as? Int ?? 0
            let of = (r["total"] as? Int).map { " of \($0)" } ?? ((r["more"] as? Bool ?? false) ? "+" : "")
            self.setStatus("✓ \(n)\(of) result\(n == 1 ? "" : "s") · \(self.lastJQL)",
                           n == 0 ? .secondaryLabelColor : .labelColor)
            self.controller?.log("jira: live search \(n) result(s): \(self.lastJQL)")
            let file = ((r["file"] as? String ?? JiraPoll.liveSearchFile) as NSString).lastPathComponent
            if let show = self.controller?.jiraShowTab {
                show(file)
            } else {
                self.controller?.pendingJiraTab = file
                self.controller?.reloadJiraWindow()
            }
            self.panel.makeKeyAndOrderFront(nil)
        }
    }

    private func maxChanged(_ v: String) {
        guard let m = Int(v), m > 0, m <= 1000 else { return }
        JiraPoll.run("jira_config.py", ["--set-live-search"], stdin: "{\"maxResults\": \(m)}") { _, _, _ in }
    }

    @objc private func fetchDirectory(_ sender: Any?) {
        fetchButton.alphaValue = 0.5
        spinner.startAnimation(nil)
        setStatus("Fetching projects, users, statuses, releases, labels… (a few calls per project)", .secondaryLabelColor)
        JiraPoll.run("jira_poll.py", ["--directory", "--quiet"]) { [weak self] code, _, err in
            guard let self else { return }
            self.spinner.stopAnimation(nil)
            self.fetchButton.alphaValue = 1
            if code == 0 {
                self.reloadLists()
                self.setStatus("✓ \(self.dir.users.count) users · \(self.dir.projects.count) projects · "
                               + "\(self.dir.versions.count) releases · \(self.dir.labels.count) labels cached",
                               .secondaryLabelColor)
            } else {
                self.setStatus("✗ " + JiraPoll.errorLine(err, fallback: "directory failed (exit \(code))"), .systemRed)
            }
        }
    }

    private func copy(_ s: String, _ what: String) {
        guard !s.isEmpty else { setStatus("Run a search first.", .secondaryLabelColor); return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
        setStatus("Copied \(what).", .secondaryLabelColor)
    }
}

// top-down document view for the search panel's scrolling filter rows
final class JiraFlippedView: NSView {
    override var isFlipped: Bool { true }
}
