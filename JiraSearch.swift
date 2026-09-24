import AppKit

// MARK: - Jira live search (Cmd+F in the Jira window) + the value pickers
//
// JiraSearchPanel: a bar docked to the Jira window (child window, above it
// when there is room, else below). Free text + Projects + "+ Filter" rows
// (assignee / reporter / status / type / priority / dates / labels / any
// column). Return runs `jira_poll.py --live-search` (criteria JSON on stdin)
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
    var projects: [(key: String, name: String)] = []
    var users: [User] = []
    var statuses: [String] = []
    var issueTypes: [String] = []
    var priorities: [String] = []
    var fields: [Field] = []
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
}

// MARK: - JiraMultiPicker

final class JiraMultiPicker: NSView, NSTableViewDataSource, NSTableViewDelegate,
                             NSSearchFieldDelegate, NSPopoverDelegate {
    struct Option { let id: String; let title: String; let detail: String }
    private static let allID = "\u{0}all"

    var options: [Option] = [] { didSet { updateDisplay() } }
    private(set) var selected: [String] = []
    private(set) var isAll = false
    var allTitle: String?          // non-nil: offers an "All …" state
    var noun = "value"
    var onChange: (() -> Void)?
    var placeholder = "Choose…" { didSet { updateDisplay() } }

    private let display = NSTokenField()
    private let chevron = NSButton()
    private var popover: NSPopover?
    private let search = NSSearchField()
    private let table = NSTableView()
    private let countLabel = NSTextField(labelWithString: "")
    private var shown: [Option] = []
    private var monitor: Any?

    init(noun: String, allTitle: String? = nil) {
        self.noun = noun
        self.allTitle = allTitle
        super.init(frame: .zero)
        display.isEditable = false
        display.isSelectable = false
        display.isBezeled = true
        display.bezelStyle = .roundedBezel
        display.font = .systemFont(ofSize: 12)
        (display.cell as? NSTokenFieldCell)?.wraps = false
        (display.cell as? NSTokenFieldCell)?.lineBreakMode = .byTruncatingTail
        chevron.bezelStyle = .inline
        chevron.isBordered = false
        chevron.image = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: "Choose")
        chevron.target = self
        chevron.action = #selector(togglePopover(_:))
        chevron.toolTip = "Choose \(noun)s (type to filter)"
        for v in [display, chevron] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            display.leadingAnchor.constraint(equalTo: leadingAnchor),
            display.topAnchor.constraint(equalTo: topAnchor),
            display.bottomAnchor.constraint(equalTo: bottomAnchor),
            display.heightAnchor.constraint(equalToConstant: 24),
            chevron.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            chevron.centerYAnchor.constraint(equalTo: centerYAnchor),
            chevron.widthAnchor.constraint(equalToConstant: 16),
            display.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
        setContentHuggingPriority(.defaultLow, for: .horizontal)
        updateDisplay()
    }
    required init?(coder: NSCoder) { fatalError() }

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
        if isAll, let a = allTitle {
            display.objectValue = [a]
        } else if selected.isEmpty {
            display.objectValue = []
            display.placeholderString = options.isEmpty ? "(no \(noun)s cached)" : placeholder
        } else {
            let t = selected.map(title(of:))
            display.objectValue = t.count > 5 ? Array(t.prefix(4)) + ["+\(t.count - 4)"] : t
        }
        display.toolTip = selected.map(title(of:)).joined(separator: ", ")
    }

    // align with form labels (NSGridView .firstBaseline rows)
    override var firstBaselineOffsetFromTop: CGFloat { display.firstBaselineOffsetFromTop }
    override var lastBaselineOffsetFromBottom: CGFloat { display.lastBaselineOffsetFromBottom }

    // clicks anywhere on the control open the list
    override func hitTest(_ point: NSPoint) -> NSView? {
        guard let v = super.hitTest(point) else { return nil }
        return v === chevron ? chevron : self
    }
    override func mouseDown(with event: NSEvent) { togglePopover(nil) }
    override var acceptsFirstResponder: Bool { true }
    override func keyDown(with event: NSEvent) {
        // Space / Return / Down on the focused control open the list
        if [49, 36, 125].contains(event.keyCode) { togglePopover(nil) } else { super.keyDown(with: event) }
    }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { NSBezierPath(roundedRect: bounds, xRadius: 5, yRadius: 5).fill() }

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
        img.contentTintColor = isOn(o) ? .controlAccentColor : .tertiaryLabelColor
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

    // criterion kinds for "+ Filter"
    private enum ValueKind { case users, list([String]), date, text }
    private struct Kind { let key: String; let title: String; let value: ValueKind }
    private final class Row {
        let kind: Kind
        let view: NSStackView
        let control: NSView
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

    private let textField = NSTextField()
    private let projects = JiraMultiPicker(noun: "project", allTitle: "All projects")
    private let addFilter = NSPopUpButton(frame: .zero, pullsDown: true)
    private let searchButton = NSButton(title: "Search", target: nil, action: nil)
    private let rowsStack = NSStackView()
    private let status = NSTextField(labelWithString: "")
    private let spinner = NSProgressIndicator()
    private let maxBox = NSComboBox()
    private let fetchButton = NSButton(title: "Fetch users & projects", target: nil, action: nil)
    private let stack = NSStackView()

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

    private init(controller: SwitcherController) {
        self.controller = controller
        panel = JiraKeyPanel(contentRect: NSRect(x: 0, y: 0, width: 760, height: 90),
                             styleMask: [.borderless], backing: .buffered, defer: false)
        super.init()
        panel.isReleasedWhenClosed = false
        panel.hasShadow = true
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hidesOnDeactivate = false
        panel.autorecalculatesKeyViewLoop = true
        build()
    }

    private func build() {
        let fx = NSVisualEffectView()
        fx.material = .popover
        fx.state = .active
        fx.blendingMode = .behindWindow
        fx.wantsLayer = true
        fx.layer?.cornerRadius = 10
        fx.layer?.masksToBounds = true
        panel.contentView = fx

        textField.placeholderString = "Search text (summary, description, comments…) — Return searches"
        textField.font = .systemFont(ofSize: 15)
        textField.bezelStyle = .roundedBezel
        textField.delegate = self
        textField.target = self
        textField.action = #selector(run(_:))
        textField.setContentHuggingPriority(.defaultLow - 1, for: .horizontal)
        projects.placeholder = "Projects…"
        projects.onChange = { [weak self] in self?.saveState() }
        projects.widthAnchor.constraint(equalToConstant: 230).isActive = true
        addFilter.bezelStyle = .rounded
        addFilter.target = self
        addFilter.action = #selector(addFilterPicked(_:))
        searchButton.bezelStyle = .rounded
        searchButton.target = self
        searchButton.action = #selector(run(_:))
        let close = NSButton(image: NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close") ?? NSImage(),
                             target: self, action: #selector(closeClicked(_:)))
        close.isBordered = false
        close.toolTip = "Close (Esc / ⌘F)"
        let top = hrow([textField, projects, addFilter, searchButton, close])

        rowsStack.orientation = .vertical
        rowsStack.alignment = .leading
        rowsStack.spacing = 6

        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail
        status.isSelectable = true
        status.setContentCompressionResistancePriority(.defaultLow - 10, for: .horizontal)
        status.setContentHuggingPriority(.defaultLow - 1, for: .horizontal)
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        maxBox.addItems(withObjectValues: Self.maxChoices)
        maxBox.controlSize = .small
        maxBox.font = .systemFont(ofSize: 11)
        maxBox.target = self
        maxBox.action = #selector(maxChanged(_:))
        maxBox.toolTip = "Max results per search (liveSearch.maxResults in config.json)"
        maxBox.widthAnchor.constraint(equalToConstant: 64).isActive = true
        let maxLabel = NSTextField(labelWithString: "Max")
        maxLabel.font = .systemFont(ofSize: 11)
        maxLabel.textColor = .secondaryLabelColor
        func small(_ t: String, _ a: Selector, _ tip: String) -> NSButton {
            let b = NSButton(title: t, target: self, action: a)
            b.bezelStyle = .rounded
            b.controlSize = .small
            b.toolTip = tip
            return b
        }
        fetchButton.bezelStyle = .rounded
        fetchButton.controlSize = .small
        fetchButton.target = self
        fetchButton.action = #selector(fetchDirectory(_:))
        fetchButton.toolTip = "Run the weekly directory job now (jira_poll.py --directory)"
        let footer = hrow([spinner, status, fetchButton, maxLabel, maxBox,
                           small("Copy JQL", #selector(copyJQL(_:)), "Copy the last search's JQL"),
                           small("Copy curl", #selector(copyCurl(_:)), "Copy the last search's request (includes the token)")])

        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 8, right: 12)
        for v in [top, rowsStack, footer] as [NSView] { stack.addArrangedSubview(v) }
        stack.translatesAutoresizingMaskIntoConstraints = false
        fx.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: fx.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: fx.trailingAnchor),
            stack.topAnchor.constraint(equalTo: fx.topAnchor),
        ])
        for v in [top, rowsStack, footer] as [NSView] {
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

    // MARK: attach / detach

    private func attach(to w: PopupWindow) {
        if let old = host, old !== w { old.nativeWindow.removeChildWindow(panel) }
        host = w
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

    @objc private func closeClicked(_ sender: Any?) { close() }

    // above the jira window when there is room, else below, else inside its top
    private func place() {
        guard let hw = host?.nativeWindow else { return }
        stack.layoutSubtreeIfNeeded()
        let h = ceil(stack.fittingSize.height)
        let f = hw.frame
        let width = max(f.width, 680)
        let vis = (hw.screen ?? NSScreen.main)?.visibleFrame ?? f
        var y = f.maxY + 6
        if y + h > vis.maxY {
            y = f.minY - h - 6
            if y < vis.minY { y = f.maxY - h - 44 }
        }
        panel.setFrame(NSRect(x: f.minX, y: y, width: width, height: h), display: true)
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
        fetchButton.isHidden = !dir.isEmpty
        if dir.isEmpty && status.stringValue.isEmpty {
            setStatus("No users / projects cached yet — the pickers fill after the weekly "
                      + "directory job runs (or Fetch now).", .secondaryLabelColor)
        }
        rebuildFilterMenu()
        if !restored {
            restored = true
            restoreState()
        }
    }

    // describe: team project keys, the column catalog (+ Filter), max results
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
            if self.maxBox.stringValue.isEmpty, let m = (d["liveSearch"] as? [String: Any])?["maxResults"] as? Int {
                self.maxBox.stringValue = String(m)
            }
            self.rebuildFilterMenu()
        }
    }

    private var kinds: [Kind] {
        var k: [Kind] = [
            Kind(key: "assignee", title: "Assignee", value: .users),
            Kind(key: "reporter", title: "Reporter", value: .users),
            Kind(key: "status", title: "Status", value: .list(dir.statuses)),
            Kind(key: "issuetype", title: "Issue type", value: .list(dir.issueTypes)),
            Kind(key: "priority", title: "Priority", value: .list(dir.priorities)),
            Kind(key: "updated", title: "Updated within", value: .date),
            Kind(key: "created", title: "Created within", value: .date),
            Kind(key: "resolved", title: "Resolved within", value: .date),
            Kind(key: "labels", title: "Labels (a, b = any)", value: .text),
            Kind(key: "fixVersion", title: "Release (a, b = any)", value: .text),
            Kind(key: "field:title", title: "Summary contains", value: .text),
            Kind(key: "field:description", title: "Description contains", value: .text),
        ]
        // every other known column (team custom fields, raw ids) as "contains"
        let skip: Set<String> = ["key", "title", "status", "assignee", "release", "releaseLabel",
                                 "releaseDate", "releaseStatus", "priority", "labels", "description",
                                 "reporter", "project", "updated", "comments"]
        for c in info["catalog"] as? [[String: Any]] ?? [] {
            guard let f = c["field"] as? String, !skip.contains(f) else { continue }
            let label = (c["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? f
            k.append(Kind(key: "field:\(f)", title: "\(label) contains", value: .text))
        }
        k.append(Kind(key: "jql", title: "Raw JQL", value: .text))
        return k
    }

    private func rebuildFilterMenu() {
        addFilter.removeAllItems()
        addFilter.addItem(withTitle: "+ Filter")
        for k in kinds {
            addFilter.addItem(withTitle: k.title)
            addFilter.lastItem?.representedObject = k.key
            addFilter.lastItem?.isEnabled = !rows.contains { $0.kind.key == k.key }
        }
        addFilter.autoenablesItems = false
    }

    // MARK: criterion rows

    @objc private func addFilterPicked(_ sender: NSPopUpButton) {
        guard let key = sender.selectedItem?.representedObject as? String,
              let k = kinds.first(where: { $0.key == key }) else { return }
        let r = addRow(k)
        if let p = r.control as? JiraMultiPicker {
            DispatchQueue.main.async { p.togglePopover(nil) }
        } else {
            panel.makeFirstResponder(r.control)
        }
        saveState()
    }

    @discardableResult
    private func addRow(_ k: Kind) -> Row {
        let label = NSTextField(labelWithString: k.title)
        label.alignment = .right
        label.textColor = .secondaryLabelColor
        label.lineBreakMode = .byTruncatingTail
        label.widthAnchor.constraint(equalToConstant: 150).isActive = true
        let control: NSView
        switch k.value {
        case .users, .list:
            let p = JiraMultiPicker(noun: k.key == "assignee" || k.key == "reporter" ? "user" : k.title.lowercased())
            p.onChange = { [weak self] in self?.saveState() }
            control = p
        case .date:
            let p = NSPopUpButton(frame: .zero, pullsDown: false)
            for r in Self.dateRanges {
                p.addItem(withTitle: Self.dateTitle(r))
                p.lastItem?.representedObject = r
            }
            p.selectItem(at: min(2, p.numberOfItems - 1))
            p.target = self
            p.action = #selector(controlChanged(_:))
            control = p
        case .text:
            let f = NSTextField()
            f.placeholderString = k.key == "jql" ? "e.g. component = API" : "value — Return searches"
            f.target = self
            f.action = #selector(run(_:))
            f.delegate = self
            control = f
        }
        control.setContentHuggingPriority(.defaultLow - 1, for: .horizontal)
        let remove = NSButton(image: NSImage(systemSymbolName: "minus.circle", accessibilityDescription: "Remove") ?? NSImage(),
                              target: self, action: #selector(removeRow(_:)))
        remove.isBordered = false
        remove.toolTip = "Remove this filter"
        let v = hrow([label, control, remove])
        let r = Row(kind: k, view: v, control: control)
        remove.identifier = NSUserInterfaceItemIdentifier(k.key)
        rows.append(r)
        fillOptions(r)
        rowsStack.addArrangedSubview(v)
        v.widthAnchor.constraint(equalTo: rowsStack.widthAnchor).isActive = true
        rebuildFilterMenu()
        if panel.isVisible { place() }
        return r
    }

    private func fillOptions(_ r: Row) {
        guard let p = r.control as? JiraMultiPicker else { return }
        switch r.kind.value {
        case .users: p.options = dir.userOptions()
        case .list: p.options = JiraDirectory.options(kindsList(r.kind.key))
        default: break
        }
    }

    private func kindsList(_ key: String) -> [String] {
        switch key {
        case "status": return dir.statuses
        case "issuetype": return dir.issueTypes
        case "priority": return dir.priorities
        default: return []
        }
    }

    @objc private func removeRow(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue, let i = rows.firstIndex(where: { $0.kind.key == key }) else { return }
        rows[i].view.removeFromSuperview()
        rows.remove(at: i)
        rebuildFilterMenu()
        place()
        saveState()
    }

    @objc private func controlChanged(_ sender: Any?) { saveState() }

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
            case let p as NSPopUpButton:
                c[r.kind.key] = p.selectedItem?.representedObject as? String ?? ""
            case let f as NSTextField:
                let v = f.stringValue.trimmingCharacters(in: .whitespaces)
                guard !v.isEmpty else { break }
                if r.kind.key.hasPrefix("field:") {
                    fields[String(r.kind.key.dropFirst(6))] = v
                } else if r.kind.key == "labels" || r.kind.key == "fixVersion" {
                    c[r.kind.key] = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                } else {
                    c[r.kind.key] = v
                }
            default: break
            }
        }
        if !fields.isEmpty { c["fields"] = fields }
        if let m = Int(maxBox.stringValue), m > 0 { c["maxResults"] = m }
        return c
    }

    private var hasSavedState: Bool { UserDefaults.standard.data(forKey: Self.stateKey) != nil }

    private func saveState() {
        var rs: [[String: Any]] = []
        for r in rows {
            var o: [String: Any] = ["key": r.kind.key]
            switch r.control {
            case let p as JiraMultiPicker: o["values"] = p.selected
            case let p as NSPopUpButton: o["value"] = p.selectedItem?.representedObject as? String ?? ""
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
            // a column row needs the catalog (not loaded yet): rebuild its kind
            let k = kinds.first { $0.key == key }
                ?? (key.hasPrefix("field:") ? Kind(key: key, title: "\(key.dropFirst(6)) contains", value: .text) : nil)
            guard let k, !rows.contains(where: { $0.kind.key == key }) else { continue }
            let r = addRow(k)
            switch r.control {
            case let p as JiraMultiPicker: p.set(o["values"] as? [String] ?? [])
            case let p as NSPopUpButton:
                if let i = p.itemArray.firstIndex(where: { $0.representedObject as? String == o["value"] as? String }) {
                    p.selectItem(at: i)
                }
            case let f as NSTextField: f.stringValue = o["value"] as? String ?? ""
            default: break
            }
        }
    }

    // MARK: actions

    private func setStatus(_ s: String, _ c: NSColor) {
        status.stringValue = s
        status.textColor = c
        status.toolTip = s
    }

    @objc private func run(_ sender: Any?) {
        let crit = criteria()
        saveState()
        guard let data = try? JSONSerialization.data(withJSONObject: crit) else { return }
        spinner.startAnimation(nil)
        searchButton.isEnabled = false
        setStatus("Searching…", .secondaryLabelColor)
        JiraPoll.run("jira_poll.py", ["--live-search"], stdin: String(decoding: data, as: UTF8.self)) { [weak self] code, out, err in
            guard let self else { return }
            self.spinner.stopAnimation(nil)
            self.searchButton.isEnabled = true
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

    @objc private func maxChanged(_ sender: Any?) {
        guard let m = Int(maxBox.stringValue), m > 0, m <= 1000 else {
            setStatus("✗ Max must be 1–1000", .systemRed)
            return
        }
        JiraPoll.run("jira_config.py", ["--set-live-search"], stdin: "{\"maxResults\": \(m)}") { _, _, _ in }
    }

    @objc private func fetchDirectory(_ sender: Any?) {
        fetchButton.isEnabled = false
        spinner.startAnimation(nil)
        setStatus("Fetching projects, users, statuses… (one call per project)", .secondaryLabelColor)
        JiraPoll.run("jira_poll.py", ["--directory", "--quiet"]) { [weak self] code, _, err in
            guard let self else { return }
            self.spinner.stopAnimation(nil)
            self.fetchButton.isEnabled = true
            if code == 0 {
                self.reloadLists()
                self.setStatus("✓ \(self.dir.users.count) users · \(self.dir.projects.count) projects cached",
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
    @objc private func copyJQL(_ sender: Any?) { copy(lastJQL, "JQL") }
    @objc private func copyCurl(_ sender: Any?) { copy(lastCurl, "curl (includes the token)") }
}
