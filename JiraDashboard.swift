import AppKit

// MARK: - Jira Config window (the one place for everything jira-poll)
//
// Menu bar "Open Jira Config Window" (also the Jira window's icon menu, and
// `workspace-switcher jira-poll dashboard`). Master–detail:
//
//   sidebar          POLL JOBS  (each endpoint in config.json, + Add Poll Job)
//                    SEARCHES   (saved searches, + Add Search)
//                    SETTINGS   (Connection, Known Columns)
//   detail           the selected item's editor
//
// Poll job / search editor: its settings, ITS OWN columns (each job writes
// one tab of the Jira window and owns that tab's table), the full JQL and the
// full curl of every request (Copy curl), Save / Revert / Delete, and Force
// Poll (jobs; warns when a poll is already running) or Run (searches: the
// results become a search-<name>.json tab).
//
// Everything shown comes from ONE python call — `jira_poll.py --describe` —
// and every edit goes through jira_config.py (--upsert-* / --delete-* /
// --set-columns), which validates before writing config.json. Config stays
// the source of truth; the window is a view + editor over it.
// Sizes / refresh: [jira] dashboard-width, dashboard-height, dashboard-refresh.

// One column list editor (a job's or a search's columns).
final class JiraColumnEditor: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    var cols: [ListColumn] = [] { didSet { table.reloadData() } }
    var meta: [String: [String: Any]] = [:]      // field -> catalog entry (apiFields, label)
    var onChange: (() -> Void)?
    let table = NSTableView()
    let addField = NSComboBox()
    let copyFrom = NSPopUpButton(frame: .zero, pullsDown: true)
    var copySources: [(String, String)] = []     // (menu title, columns spec)
    private(set) var view = NSView()

    private static let spec: [(id: String, title: String, width: CGFloat)] = [
        ("field", "Field", 130), ("title", "Title", 120), ("width", "Width %", 58),
        ("align", "Align", 78), ("sort", "Sort", 38), ("filter", "Filter", 42),
        ("api", "Fetches (API field)", 150), ("label", "Custom field label", 150),
    ]

    override init() {
        super.init()
        for c in Self.spec {
            let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(c.id))
            col.title = c.title
            col.width = c.width
            col.minWidth = 30
            table.addTableColumn(col)
        }
        table.dataSource = self
        table.delegate = self
        table.rowHeight = 24
        table.usesAlternatingRowBackgroundColors = true
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        let sv = NSScrollView()
        sv.documentView = table
        sv.hasVerticalScroller = true
        sv.hasHorizontalScroller = true
        sv.borderType = .bezelBorder

        addField.placeholderString = "add a field (created, duedate, customfield_10010, …)"
        addField.completes = true
        addField.target = self
        addField.action = #selector(add(_:))
        copyFrom.addItem(withTitle: "Copy columns from…")
        copyFrom.target = self
        copyFrom.action = #selector(copyColumns(_:))
        func btn(_ t: String, _ a: Selector, _ tip: String? = nil) -> NSButton {
            let b = NSButton(title: t, target: self, action: a)
            b.bezelStyle = .rounded
            b.controlSize = .small
            b.toolTip = tip
            return b
        }
        copyFrom.controlSize = .small
        addField.controlSize = .small
        let bar = NSStackView(views: [addField, btn("Add", #selector(add(_:))),
                                      btn("Remove", #selector(remove(_:))),
                                      btn("◀", #selector(up(_:)), "Move left"),
                                      btn("▶", #selector(down(_:)), "Move right"), copyFrom])
        bar.orientation = .horizontal
        bar.spacing = 6
        bar.setHuggingPriority(.required, for: .vertical)
        addField.widthAnchor.constraint(equalToConstant: 280).isActive = true
        let stack = NSStackView(views: [sv, bar])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.distribution = .fill
        stack.spacing = 6
        sv.translatesAutoresizingMaskIntoConstraints = false
        sv.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        sv.heightAnchor.constraint(greaterThanOrEqualToConstant: 110).isActive = true
        sv.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        view = stack
    }

    func setCatalog(_ fields: [String], sources: [(String, String)]) {
        addField.removeAllItems()
        addField.addItems(withObjectValues: fields.filter { f in !cols.contains { $0.field == f } })
        copySources = sources
        while copyFrom.numberOfItems > 1 { copyFrom.removeItem(at: 1) }
        for (t, _) in sources { copyFrom.addItem(withTitle: t) }
    }

    private func changed() { table.reloadData(); onChange?() }

    // MARK: table
    func numberOfRows(in tableView: NSTableView) -> Int { cols.count }

    private func cell(_ v: NSView) -> NSView {
        let c = NSTableCellView()
        v.translatesAutoresizingMaskIntoConstraints = false
        c.addSubview(v)
        NSLayoutConstraint.activate([
            v.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 3),
            v.trailingAnchor.constraint(equalTo: c.trailingAnchor, constant: -3),
            v.centerYAnchor.constraint(equalTo: c.centerYAnchor),
        ])
        return c
    }

    private func label(_ s: String, dim: Bool = false, mono: Bool = false) -> NSTextField {
        let f = NSTextField(labelWithString: s)
        f.lineBreakMode = .byTruncatingTail
        if dim { f.textColor = .secondaryLabelColor }
        if mono { f.font = .monospacedSystemFont(ofSize: 11, weight: .regular) }
        f.toolTip = s
        return f
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let id = tableColumn?.identifier.rawValue, row < cols.count else { return nil }
        let c = cols[row]
        func edit(_ s: String, _ tag: String) -> NSView {
            let f = NSTextField(string: s)
            f.isBordered = false
            f.drawsBackground = false
            f.identifier = NSUserInterfaceItemIdentifier(tag)
            f.tag = row
            f.delegate = self
            return cell(f)
        }
        func check(_ on: Bool, _ tag: String) -> NSView {
            let b = NSButton(checkboxWithTitle: "", target: self, action: #selector(flag(_:)))
            b.state = on ? .on : .off
            b.identifier = NSUserInterfaceItemIdentifier(tag)
            b.tag = row
            return cell(b)
        }
        let m = meta[c.field] ?? [:]
        switch id {
        case "field": return cell(label(c.field, mono: true))
        case "title": return edit(c.title, "title")
        case "width": return edit(c.width == c.width.rounded() ? String(Int(c.width)) : String(format: "%.1f", c.width), "width")
        case "align":
            let p = NSPopUpButton(frame: .zero, pullsDown: false)
            p.controlSize = .small
            p.addItems(withTitles: ["left", "center", "right"])
            p.selectItem(withTitle: c.align)
            p.tag = row
            p.target = self
            p.action = #selector(align(_:))
            return cell(p)
        case "sort": return check(c.sortable, "sort")
        case "filter": return check(c.filterable, "filter")
        case "api":
            let api = (m["apiFields"] as? [String]).map { $0.isEmpty ? "(key)" : $0.joined(separator: ", ") }
            return cell(label(api ?? c.field, dim: true, mono: true))
        case "label": return cell(label(m["label"] as? String ?? "", dim: true))
        default: return nil
        }
    }

    // commit a title / width edit (Return, Tab or focus loss)
    func controlTextDidEndEditing(_ obj: Notification) {
        guard let f = obj.object as? NSTextField, f.tag < cols.count else { return }
        let r = f.tag
        let v = f.stringValue.trimmingCharacters(in: .whitespaces)
        switch f.identifier?.rawValue {
        case "title":
            // ':' and ',' are the columns-line separators
            let t = v.replacingOccurrences(of: ":", with: " ").replacingOccurrences(of: ",", with: " ")
            let nt = t.isEmpty ? cols[r].field : t
            guard nt != cols[r].title else { return }
            cols[r].title = nt
        case "width":
            guard let w = Double(v), w >= 0, w <= 100 else { table.reloadData(); return }
            guard CGFloat(w) != cols[r].width else { return }
            cols[r].width = CGFloat(w)
        default: return
        }
        changed()
    }

    @objc private func flag(_ b: NSButton) {
        guard b.tag < cols.count else { return }
        if b.identifier?.rawValue == "sort" { cols[b.tag].sortable = b.state == .on }
        else { cols[b.tag].filterable = b.state == .on }
        changed()
    }

    @objc private func align(_ p: NSPopUpButton) {
        guard p.tag < cols.count, let a = p.titleOfSelectedItem else { return }
        cols[p.tag].align = a
        changed()
    }

    @objc func add(_ sender: Any?) {
        let f = addField.stringValue.trimmingCharacters(in: .whitespaces)
            .replacingOccurrences(of: ":", with: "").replacingOccurrences(of: ",", with: "")
        guard !f.isEmpty, !cols.contains(where: { $0.field == f }) else { return }
        let lbl = (meta[f]?["label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        let title = lbl ?? ((meta[f]?["titles"] as? [String])?.first ?? f)
        cols.append(ListColumn(field: f, title: title, width: 0, align: "left", sortable: true, filterable: true))
        addField.stringValue = ""
        table.selectRowIndexes(IndexSet(integer: cols.count - 1), byExtendingSelection: false)
        table.scrollRowToVisible(cols.count - 1)
        changed()
    }

    @objc private func remove(_ sender: Any?) {
        let r = table.selectedRow
        guard r >= 0, r < cols.count, cols.count > 1 else { return }
        cols.remove(at: r)
        changed()
    }

    private func move(_ d: Int) {
        let r = table.selectedRow, to = r + d
        guard r >= 0, to >= 0, to < cols.count else { return }
        cols.swapAt(r, to)
        changed()
        table.selectRowIndexes(IndexSet(integer: to), byExtendingSelection: false)
    }
    @objc private func up(_ sender: Any?) { move(-1) }
    @objc private func down(_ sender: Any?) { move(1) }

    @objc private func copyColumns(_ sender: NSPopUpButton) {
        let i = sender.indexOfSelectedItem - 1
        guard i >= 0, i < copySources.count else { return }
        cols = ListColumn.parse(copySources[i].1)
        changed()
    }
}

final class JiraDashboardWindow: NSObject, NSWindowDelegate, NSTableViewDataSource,
                                 NSTableViewDelegate {
    private static var live: JiraDashboardWindow?

    private enum Item: Equatable {
        case group(String), job(String), addJob, search(String), addSearch, connection, known
    }

    private weak var controller: SwitcherController?
    private let window: NSWindow
    private var monitor: Any?
    private var timer: Timer?
    private var describing = false

    // data (jira_poll.py --describe)
    private var info: [String: Any] = [:]
    private var eps: [[String: Any]] = []
    private var srs: [[String: Any]] = []
    private var catalog: [[String: Any]] = []
    private var items: [Item] = []
    private var current: Item = .connection
    private var didInitialSelect = false

    // header
    private let statusLine = NSTextField(labelWithString: "Loading…")
    private let problemsLine = NSTextField(wrappingLabelWithString: "")
    private let enableButton = NSButton(title: "Enable Jira", target: nil, action: nil)
    private let stopButton = NSButton(title: "Stop Poll", target: nil, action: nil)
    private let openMenu = NSPopUpButton(frame: .zero, pullsDown: true)

    // layout
    private let sidebar = NSTableView()
    private let detailHost = NSView()

    // editor state (job or search)
    private var isNew = false
    private var dirty = false
    private var isSearch = false
    private let colEditor = JiraColumnEditor()
    private let nameField = NSTextField()
    private let fileLabel = NSTextField(labelWithString: "")
    private let typePopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let everyBox = NSComboBox()
    private let projectsField = NSTextField()
    private let queryPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let jqlField = NSTextField()
    private let argsField = NSTextField()
    private let enabledCheck = NSButton(checkboxWithTitle: "Scheduled (poll on its interval)", target: nil, action: nil)
    private let kindPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let valueField = NSTextField()
    private let editorStatus = NSTextField(wrappingLabelWithString: "")
    private let editorMsg = NSTextField(wrappingLabelWithString: "")
    private let requestText = NSTextView()
    private let saveButton = NSButton(title: "Save", target: nil, action: nil)
    private let revertButton = NSButton(title: "Revert", target: nil, action: nil)
    private let deleteButton = NSButton(title: "Delete…", target: nil, action: nil)
    private let actionButton = NSButton(title: "Force Poll", target: nil, action: nil)
    private let copyCurlButton = NSButton(title: "Copy curl", target: nil, action: nil)

    // connection / known columns
    private let connText = NSTextView()
    private let connResult = NSTextField(wrappingLabelWithString: "")
    private let knownTable = NSTableView()

    static func show(controller: SwitcherController) {
        if let w = live {
            w.refresh()
            NSApp.activate(ignoringOtherApps: true)
            w.window.makeKeyAndOrderFront(nil)
            return
        }
        let w = JiraDashboardWindow(controller: controller)
        live = w
        NSApp.activate(ignoringOtherApps: true)
        w.window.center()
        w.window.makeKeyAndOrderFront(nil)
        w.refresh()
        w.startTimer()
    }

    private init(controller: SwitcherController) {
        self.controller = controller
        let W = CGFloat(Double(jiraConfigValue("dashboard-width") ?? "") ?? 1080)
        let H = CGFloat(Double(jiraConfigValue("dashboard-height") ?? "") ?? 720)
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: W, height: H),
                          styleMask: [.titled, .closable, .resizable, .miniaturizable],
                          backing: .buffered, defer: false)
        super.init()
        window.title = "Jira Config"
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 860, height: 520)
        // same level as the setup window: above the popup windows, which
        // float at .popUpMenu
        window.level = NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 1)
        window.delegate = self
        window.contentView = buildContent()
        colEditor.onChange = { [weak self] in self?.markDirty() }
        installKeys()
    }

    // MARK: layout helpers

    @discardableResult
    private func button(_ b: NSButton, _ action: Selector, tip: String? = nil) -> NSButton {
        b.bezelStyle = .rounded
        b.target = self
        b.action = action
        b.toolTip = tip
        return b
    }

    private func row(_ views: [NSView], spacing: CGFloat = 8) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .horizontal
        s.spacing = spacing
        s.alignment = .centerY
        s.setHuggingPriority(.required, for: .vertical)
        return s
    }

    private func vstack(_ views: [NSView], spacing: CGFloat = 8) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .vertical
        s.alignment = .leading
        s.distribution = .fill
        s.spacing = spacing
        for v in views { v.translatesAutoresizingMaskIntoConstraints = false }
        return s
    }

    private func pinned(_ v: NSView, in parent: NSView, inset: CGFloat = 0) {
        v.translatesAutoresizingMaskIntoConstraints = false
        parent.addSubview(v)
        NSLayoutConstraint.activate([
            v.leadingAnchor.constraint(equalTo: parent.leadingAnchor, constant: inset),
            v.trailingAnchor.constraint(equalTo: parent.trailingAnchor, constant: -inset),
            v.topAnchor.constraint(equalTo: parent.topAnchor, constant: inset),
            v.bottomAnchor.constraint(equalTo: parent.bottomAnchor, constant: -inset),
        ])
    }

    private func monoTextView(_ tv: NSTextView) -> NSScrollView {
        let sv = NSScrollView()
        sv.hasVerticalScroller = true
        sv.borderType = .bezelBorder
        tv.isEditable = false
        tv.isSelectable = true
        tv.isRichText = true
        tv.font = .monospacedSystemFont(ofSize: 11.5, weight: .regular)
        tv.textContainerInset = NSSize(width: 6, height: 6)
        tv.isVerticallyResizable = true
        tv.autoresizingMask = [.width]
        tv.textContainer?.widthTracksTextView = true   // wrap: nothing is cut off
        sv.documentView = tv
        return sv
    }

    private func sectionTitle(_ s: String) -> NSTextField {
        let l = NSTextField(labelWithString: s)
        l.font = .systemFont(ofSize: 11, weight: .semibold)
        l.textColor = .secondaryLabelColor
        return l
    }

    private func buildContent() -> NSView {
        let content = NSView()
        statusLine.font = .systemFont(ofSize: 13, weight: .semibold)
        statusLine.lineBreakMode = .byTruncatingTail
        problemsLine.font = .systemFont(ofSize: 11)
        problemsLine.textColor = .systemRed
        problemsLine.isHidden = true
        openMenu.addItem(withTitle: "Open…")
        openMenu.bezelStyle = .rounded
        openMenu.target = self
        openMenu.action = #selector(openFile(_:))
        openMenu.toolTip = "Open a jira file in the notes window"
        stopButton.isHidden = true
        let header = row([
            button(enableButton, #selector(toggleEnabled(_:)), tip: "[jira] enabled — the Jira window + the launchd poll agent"),
            button(NSButton(title: "Setup…", target: nil, action: nil), #selector(setup(_:)), tip: "Site, token, auth"),
            openMenu,
            button(stopButton, #selector(stopPoll(_:)), tip: "Stop the running poll (jira_poll.py --cancel)"),
        ])
        let top = vstack([statusLine, problemsLine, header], spacing: 6)
        top.setHuggingPriority(.required, for: .vertical)
        for v in [statusLine, problemsLine] as [NSView] { v.setContentHuggingPriority(.required, for: .vertical) }

        // sidebar: a source list — reads as navigation, not as buttons
        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("item"))
        col.width = 200
        sidebar.addTableColumn(col)
        sidebar.headerView = nil
        sidebar.style = .sourceList
        sidebar.rowHeight = 26
        sidebar.dataSource = self
        sidebar.delegate = self
        let sideSV = NSScrollView()
        sideSV.documentView = sidebar
        sideSV.hasVerticalScroller = true
        sideSV.drawsBackground = false

        let split = NSSplitView()
        split.isVertical = true
        split.dividerStyle = .thin
        split.addArrangedSubview(sideSV)
        split.addArrangedSubview(detailHost)
        split.setHoldingPriority(.defaultLow + 10, forSubviewAt: 0)
        sideSV.widthAnchor.constraint(greaterThanOrEqualToConstant: 170).isActive = true
        detailHost.widthAnchor.constraint(greaterThanOrEqualToConstant: 560).isActive = true

        top.translatesAutoresizingMaskIntoConstraints = false
        split.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(top)
        content.addSubview(split)
        NSLayoutConstraint.activate([
            top.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            top.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            top.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            statusLine.widthAnchor.constraint(equalTo: top.widthAnchor),
            problemsLine.widthAnchor.constraint(equalTo: top.widthAnchor),
            split.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            split.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            split.topAnchor.constraint(equalTo: top.bottomAnchor, constant: 10),
            split.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
        DispatchQueue.main.async { split.setPosition(210, ofDividerAt: 0) }
        setupEditorControls()
        return content
    }

    private func setupEditorControls() {
        typePopup.addItems(withTitles: ["issues", "releases"])
        everyBox.addItems(withObjectValues: JiraPoll.intervals)
        everyBox.completes = true
        projectsField.placeholderString = "*  (all)   or   KEY1, KEY2"
        jqlField.placeholderString = "extra JQL, ANDed with the time window (e.g. assignee = currentUser())"
        argsField.placeholderString = "name=value, name=value"
        valueField.placeholderString = "value"
        for f in [nameField, projectsField, jqlField, argsField, valueField, everyBox] as [NSTextField] {
            NotificationCenter.default.addObserver(self, selector: #selector(textDidChange(_:)),
                                                   name: NSControl.textDidChangeNotification, object: f)
        }
        everyBox.target = self
        everyBox.action = #selector(fieldChanged(_:))
        for p in [typePopup, queryPopup, kindPopup] {
            p.target = self
            p.action = #selector(popupChanged(_:))
        }
        enabledCheck.target = self
        enabledCheck.action = #selector(fieldChanged(_:))
        editorStatus.font = .systemFont(ofSize: 12)
        editorMsg.font = .systemFont(ofSize: 12)
        fileLabel.textColor = .secondaryLabelColor
        fileLabel.font = .monospacedSystemFont(ofSize: 11.5, weight: .regular)
        button(saveButton, #selector(save(_:)), tip: "Validate + write config.json (⌘S)")
        button(revertButton, #selector(revert(_:)))
        button(deleteButton, #selector(deleteItem(_:)))
        button(actionButton, #selector(primaryAction(_:)))
        button(copyCurlButton, #selector(copyCurl(_:)), tip: "Copy every request as curl (real token)")
    }

    // MARK: keys (rule.md #1: edit shortcuts in every field)

    private func installKeys() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.window.isKeyWindow, self.window.attachedSheet == nil else { return e }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command), ctrl = mods.contains(.control)
            let editing = self.window.firstResponder is NSTextView
                && (self.window.firstResponder as? NSTextView)?.isEditable == true
            if e.keyCode == 53 {                       // Esc: end an edit, else close
                if editing { self.window.makeFirstResponder(nil); return nil }
                self.close()
                return nil
            }
            if cmd && e.keyCode == 13 { self.close(); return nil }           // Cmd+W
            if cmd && e.keyCode == 15 { self.refresh(); return nil }         // Cmd+R
            if cmd && e.keyCode == 1 { self.save(nil); return nil }          // Cmd+S
            guard cmd || ctrl, let ed = self.window.firstResponder as? NSText else { return e }
            switch e.keyCode {
            case 9: ed.paste(nil)                        // Cmd+V / Ctrl+V
            case 8: ed.copy(nil)                         // Cmd+C / Ctrl+C
            case 0 where cmd: ed.selectAll(nil)          // Cmd+A
            case 7 where cmd: ed.cut(nil)                // Cmd+X
            case 6 where cmd: ed.undoManager?.undo()     // Cmd+Z
            default: return e
            }
            return nil
        }
    }

    // MARK: data

    private func startTimer() {
        let secs = max(2, Double(jiraConfigValue("dashboard-refresh") ?? "") ?? 5)
        timer = Timer.scheduledTimer(withTimeInterval: secs, repeats: true) { [weak self] _ in
            guard let self, self.window.isVisible else { return }
            self.refresh()
        }
    }

    func refresh(then: (() -> Void)? = nil) {
        guard !describing else { return }
        describing = true
        JiraPoll.run("jira_poll.py", ["--describe"]) { [weak self] code, out, err in
            guard let self else { return }
            self.describing = false
            guard let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] else {
                self.statusLine.stringValue = "✗ jira_poll.py --describe failed: "
                    + JiraPoll.errorLine(err, fallback: "exit \(code)")
                self.statusLine.textColor = .systemRed
                return
            }
            self.apply(d)
            then?()
        }
    }

    private func apply(_ d: [String: Any]) {
        info = d
        eps = d["endpoints"] as? [[String: Any]] ?? []
        srs = d["searches"] as? [[String: Any]] ?? []
        catalog = d["catalog"] as? [[String: Any]] ?? []
        var its: [Item] = [.group("POLL JOBS")]
        its += eps.compactMap { ($0["name"] as? String).map(Item.job) }
        its += [.addJob, .group("SEARCHES")]
        its += srs.compactMap { ($0["name"] as? String).map(Item.search) }
        its += [.addSearch, .group("SETTINGS"), .connection, .known]
        items = its
        sidebar.reloadData()
        if !didInitialSelect {
            didInitialSelect = true
            current = eps.first.flatMap { ($0["name"] as? String).map(Item.job) } ?? .connection
            showItem(current)
        } else if !items.contains(current) {
            current = .connection
            showItem(current)
        } else {
            updateLive()
            updateButtons()
        }
        if let i = items.firstIndex(of: current) {
            sidebar.selectRowIndexes(IndexSet(integer: i), byExtendingSelection: false)
        }
        updateHeader()
    }

    private var lockHeld: Bool { (info["lock"] as? [String: Any])?["held"] as? Bool ?? false }

    private func updateHeader() {
        let enabled = info["enabled"] as? Bool ?? jiraEnabledInConfig()
        let bg = info["backgroundPoll"] as? Bool ?? false
        let lock = info["lock"] as? [String: Any] ?? [:]
        var parts = [enabled ? "● Polling ON" : bg ? "◐ Jira disabled — background polling" : "○ Polling OFF"]
        parts.append("\(eps.count) job\(eps.count == 1 ? "" : "s") · \(srs.count) search\(srs.count == 1 ? "" : "es")")
        parts.append("launchd tick \(info["tick"] as? String ?? "60s")")
        if let lr = info["lastRun"] as? String, !lr.isEmpty {
            parts.append("last run \(JiraPoll.short(lr)) \(info["status"] as? String ?? "")")
        }
        if lockHeld { parts.append("⟳ polling now (pid \(lock["pid"] ?? "?"), since \(JiraPoll.short(lock["since"] as? String)))") }
        statusLine.stringValue = parts.joined(separator: "  ·  ")
        statusLine.textColor = enabled ? .labelColor : .secondaryLabelColor
        var probs = info["problems"] as? [String] ?? []
        if let e = info["lastError"] as? String, !e.isEmpty { probs.append("last error: \(e)") }
        if let e = JiraPoll.lastEnableError { probs.append("enable failed: \(e)") }
        problemsLine.stringValue = probs.map { "⚠ " + $0 }.joined(separator: "\n")
        problemsLine.isHidden = probs.isEmpty
        enableButton.title = enabled ? "Disable Jira" : "Enable Jira"
        stopButton.isHidden = !(lockHeld || !JiraPoll.running.isEmpty)

        while openMenu.numberOfItems > 1 { openMenu.removeItem(at: 1) }
        let fm = FileManager.default
        let files: [(String, String?)] = [
            ("Jira config (config.json)", info["configPath"] as? String ?? JiraPoll.configPath),
            ("Team schema (team.json)", info["teamPath"] as? String),
            ("commands.conf", info["commandsConf"] as? String),
            ("Poll status (status.json)", info["statusPath"] as? String ?? JiraPoll.statusPath),
            ("curl log", info["curlLog"] as? String ?? JiraPoll.curlLogPath),
        ]
        for (title, path) in files {
            guard let path else { continue }
            let exists = fm.fileExists(atPath: path)
            let item = NSMenuItem(title: exists ? title : "\(title) — create from example",
                                  action: nil, keyEquivalent: "")
            item.representedObject = path
            item.isEnabled = exists || title.hasPrefix("Team")
            item.toolTip = path
            openMenu.menu?.addItem(item)
        }
    }

    // MARK: sidebar

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === sidebar ? items.count : catalog.count
    }

    func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
        guard tableView === sidebar, row < items.count, case .group = items[row] else { return false }
        return true
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        guard tableView === sidebar else { return true }
        guard row < items.count else { return false }
        if case .group = items[row] { return false }
        if items[row] == current || !dirty { return true }
        // unsaved edits: ask first (a sheet), then select programmatically
        confirmDiscard { [weak self] in
            self?.sidebar.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
        return false
    }

    private func statusColor(_ st: String) -> NSColor {
        st == "ok" ? .systemGreen : st == "error" ? .systemRed
            : st == "running" ? .systemOrange : .tertiaryLabelColor
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if tableView === knownTable { return knownCell(tableColumn?.identifier.rawValue ?? "", row) }
        guard row < items.count else { return nil }
        let c = NSTableCellView()
        let l = NSTextField(labelWithString: "")
        l.lineBreakMode = .byTruncatingTail
        var dot: NSColor?
        switch items[row] {
        case .group(let t):
            l.stringValue = t
            l.font = .systemFont(ofSize: 11, weight: .semibold)
            l.textColor = .secondaryLabelColor
        case .job(let n):
            let e = eps.first { $0["name"] as? String == n } ?? [:]
            var st = e["status"] as? String ?? ""
            if JiraPoll.running.contains(n) || JiraPoll.running.contains("*") { st = "running" }
            dot = statusColor(st)
            let off = (e["enabled"] as? Bool ?? true) ? "" : "  (off)"
            l.stringValue = "\(n)  ·  \(e["window"] as? String ?? "")\(off)"
        case .search(let n):
            let s = srs.first { $0["name"] as? String == n } ?? [:]
            var st = s["status"] as? String ?? ""
            if JiraPoll.running.contains("search:\(n)") { st = "running" }
            dot = statusColor(st)
            l.stringValue = n
        case .addJob:
            l.stringValue = "+ Add Poll Job"
            l.textColor = .controlAccentColor
        case .addSearch:
            l.stringValue = "+ Add Search"
            l.textColor = .controlAccentColor
        case .connection: l.stringValue = "Connection"
        case .known: l.stringValue = "Known Columns"
        }
        var views: [NSView] = []
        if let dot {
            let d = NSTextField(labelWithString: "●")
            d.textColor = dot
            d.font = .systemFont(ofSize: 9)
            views.append(d)
        }
        views.append(l)
        let s = NSStackView(views: views)
        s.spacing = 6
        s.translatesAutoresizingMaskIntoConstraints = false
        c.addSubview(s)
        NSLayoutConstraint.activate([
            s.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 4),
            s.trailingAnchor.constraint(lessThanOrEqualTo: c.trailingAnchor, constant: -4),
            s.centerYAnchor.constraint(equalTo: c.centerYAnchor),
        ])
        return c
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard (notification.object as? NSTableView) === sidebar else { return }
        let r = sidebar.selectedRow
        guard r >= 0, r < items.count, items[r] != current else { return }
        current = items[r]
        showItem(current)
    }

    // alerts are SHEETS on this window: an app-modal NSAlert opens at the
    // normal level, i.e. hidden behind this window (it floats above the
    // popups). `then(true)` = the first button.
    private func ask(_ a: NSAlert, then: @escaping (Bool) -> Void) {
        a.beginSheetModal(for: window) { r in then(r == .alertFirstButtonReturn) }
    }

    // run `then` now, or after the user agrees to drop unsaved edits
    private func confirmDiscard(then: @escaping () -> Void) {
        guard dirty else { then(); return }
        let a = NSAlert()
        a.messageText = "Discard unsaved changes?"
        a.informativeText = "The edits to this \(isSearch ? "search" : "poll job") are not saved."
        a.addButton(withTitle: "Discard")
        a.addButton(withTitle: "Keep Editing")
        ask(a) { [weak self] ok in
            guard ok, let self else { return }
            self.dirty = false
            then()
        }
    }

    // MARK: detail pages

    private func showPage(_ v: NSView) {
        detailHost.subviews.forEach { $0.removeFromSuperview() }
        pinned(v, in: detailHost, inset: 14)
    }

    private func showItem(_ it: Item) {
        dirty = false
        switch it {
        case .job(let n): showEditor(search: false, data: eps.first { $0["name"] as? String == n }, new: false)
        case .search(let n): showEditor(search: true, data: srs.first { $0["name"] as? String == n }, new: false)
        case .addJob: showEditor(search: false, data: nil, new: true)
        case .addSearch: showEditor(search: true, data: nil, new: true)
        case .connection: showConnection()
        case .known: showKnown()
        case .group: break
        }
    }

    private var editingName: String? {
        switch current {
        case .job(let n), .search(let n): return n
        default: return nil
        }
    }
    private var liveData: [String: Any]? {
        guard let n = editingName else { return nil }
        return (isSearch ? srs : eps).first { $0["name"] as? String == n }
    }

    // label/control form; controls keep their natural height
    private var gridView: NSGridView?
    private func grid(_ rows: [(String, NSView)]) -> NSGridView {
        let g = NSGridView(views: rows.map { r -> [NSView] in
            let l = NSTextField(labelWithString: r.0)
            l.alignment = .right
            l.textColor = .secondaryLabelColor
            return [l, r.1]
        })
        g.rowSpacing = 7
        g.columnSpacing = 10
        g.column(at: 0).xPlacement = .trailing
        g.rowAlignment = .firstBaseline
        g.setContentHuggingPriority(.required, for: .vertical)
        return g
    }

    private func setRowHidden(_ v: NSView, _ hidden: Bool) {
        guard let g = gridView, let cell = g.cell(for: v) else { return }
        cell.row?.isHidden = hidden
    }

    private func setRowLabel(_ v: NSView, _ text: String) {
        guard let g = gridView, let row = g.cell(for: v)?.row,
              let l = row.cell(at: 0).contentView as? NSTextField else { return }
        l.stringValue = text
    }

    private func showEditor(search: Bool, data: [String: Any]?, new: Bool) {
        isSearch = search
        isNew = new
        let d = data ?? [:]
        nameField.stringValue = d["name"] as? String ?? ""
        nameField.isEditable = new
        nameField.isSelectable = true
        nameField.placeholderString = search ? "e.g. my-reported" : "e.g. team-bugs"
        let projects: String = {
            if let p = d["projects"] as? [String] { return p.joined(separator: ", ") }
            return search ? "" : "*"
        }()
        projectsField.stringValue = projects
        let template = info["columnsTemplate"] as? String ?? ""
        var meta: [String: [String: Any]] = [:]
        for c in catalog { if let f = c["field"] as? String { meta[f] = c } }
        colEditor.meta = meta
        colEditor.cols = ListColumn.parse((d["columnsSpec"] as? String) ?? template)
        let me = d["name"] as? String
        var sources: [(String, String)] = [("[jira] columns (starter template)", template)]
        for e in eps where search || (e["name"] as? String) != me {
            sources.append(("job: \(e["name"] as? String ?? "")", e["columnsSpec"] as? String ?? ""))
        }
        for s in srs where !search || (s["name"] as? String) != me {
            sources.append(("search: \(s["name"] as? String ?? "")", s["columnsSpec"] as? String ?? ""))
        }
        colEditor.setCatalog(info["availableFields"] as? [String] ?? [], sources: sources)

        var form: [(String, NSView)] = [("Name", nameField), ("Writes tab", fileLabel)]
        if search {
            kindPopup.removeAllItems()
            let kinds = info["searchKinds"] as? [[String: Any]] ?? []
            for k in kinds {
                kindPopup.addItem(withTitle: k["label"] as? String ?? "")
                kindPopup.lastItem?.representedObject = k
            }
            let kind = d["kind"] as? String ?? "text"
            if let i = kinds.firstIndex(where: { $0["kind"] as? String == kind }) { kindPopup.selectItem(at: i) }
            let args = d["args"] as? [String: Any] ?? [:]
            let argName = selectedKind["arg"] as? String ?? "query"
            valueField.stringValue = args[argName] as? String ?? ""
            argsField.stringValue = args.filter { $0.key != argName }
                .map { "\($0.key)=\($0.value)" }.sorted().joined(separator: ", ")
            form += [("Search by", kindPopup), ("Value", valueField),
                     ("Projects", projectsField), ("Job args", argsField)]
            actionButton.title = "Run"
            actionButton.toolTip = "Run this search now — the results become its tab in the Jira window"
        } else {
            typePopup.selectItem(withTitle: d["type"] as? String ?? "issues")
            everyBox.stringValue = d["window"] as? String ?? "30m"
            enabledCheck.state = (d["enabled"] as? Bool ?? true) ? .on : .off
            queryPopup.removeAllItems()
            queryPopup.addItem(withTitle: "Everything updated in the window")
            queryPopup.addItem(withTitle: "Custom JQL")
            for j in info["teamJobs"] as? [String] ?? [] { queryPopup.addItem(withTitle: "team.json: \(j)") }
            let job = d["job"] as? String ?? ""
            let jql = d["extraJql"] as? String ?? ""
            if !job.isEmpty, queryPopup.item(withTitle: "team.json: \(job)") != nil {
                queryPopup.selectItem(withTitle: "team.json: \(job)")
                jqlField.stringValue = ""
            } else if !jql.isEmpty {
                queryPopup.selectItem(at: 1)
                jqlField.stringValue = jql
            } else {
                queryPopup.selectItem(at: 0)
                jqlField.stringValue = ""
            }
            argsField.stringValue = (d["args"] as? [String: Any] ?? [:])
                .map { "\($0.key)=\($0.value)" }.sorted().joined(separator: ", ")
            form += [("Type", typePopup), ("Every", everyBox), ("", enabledCheck),
                     ("Projects", projectsField), ("Query", queryPopup), ("JQL", jqlField),
                     ("Job args", argsField)]
            actionButton.title = "Force Poll"
            actionButton.toolTip = "Poll this job now (warns if a poll is already running)"
        }
        // a control lives in one grid at a time: detach from the previous page
        for (_, v) in form { v.removeFromSuperview() }
        let g = grid(form)
        gridView = g
        for (_, v) in form where v is NSTextField && v !== fileLabel {
            v.widthAnchor.constraint(greaterThanOrEqualToConstant: 380).isActive = true
        }
        let colsTitle = sectionTitle("COLUMNS — this \(search ? "search's" : "job's") tab in the Jira window "
                                     + "(and the fields it fetches)")
        let reqTitle = sectionTitle("REQUEST — full JQL + curl (includes the token)")
        let reqSV = monoTextView(requestText)
        reqSV.heightAnchor.constraint(greaterThanOrEqualToConstant: 100).isActive = true
        reqSV.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow - 30, for: .horizontal)
        for b in [saveButton, revertButton, deleteButton, copyCurlButton, actionButton] { b.removeFromSuperview() }
        let buttons = row([saveButton, revertButton, deleteButton, spacer, copyCurlButton, actionButton])
        deleteButton.isHidden = new
        editorMsg.stringValue = ""
        editorStatus.removeFromSuperview()
        editorMsg.removeFromSuperview()
        colEditor.view.removeFromSuperview()
        let page = vstack([editorStatus, g, colsTitle, colEditor.view, reqTitle, reqSV, editorMsg, buttons],
                          spacing: 8)
        for v in [editorStatus, colEditor.view, reqSV, editorMsg, buttons] as [NSView] {
            v.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        }
        colEditor.view.heightAnchor.constraint(equalTo: reqSV.heightAnchor, multiplier: 1.3).isActive = true
        showPage(page)
        updateFormVisibility()
        updateLive()
        updateButtons()
        if new { window.makeFirstResponder(nameField) }
    }

    private var selectedKind: [String: Any] { kindPopup.selectedItem?.representedObject as? [String: Any] ?? [:] }

    private func updateFormVisibility() {
        let name = nameField.stringValue.trimmingCharacters(in: .whitespaces)
        fileLabel.stringValue = name.isEmpty ? "(set a name)"
            : (isSearch ? "search-\(name).json" : "\(name).json")
        if isSearch {
            let kind = selectedKind["kind"] as? String ?? ""
            setRowLabel(valueField, selectedKind["argLabel"] as? String ?? "Value")
            setRowHidden(argsField, kind != "job")
            setRowHidden(projectsField, kind == "project" || kind == "jql")
            valueField.placeholderString = kind == "jql" ? "project = ABC AND status = Open"
                : kind == "job" ? "team.json job key" : "value"
        } else {
            let q = queryPopup.indexOfSelectedItem
            let releases = typePopup.titleOfSelectedItem == "releases"
            setRowHidden(queryPopup, releases)
            setRowHidden(jqlField, releases || q != 1)
            setRowHidden(argsField, releases || q < 2)
        }
    }

    // status line + request text from the latest describe (never touches the form)
    private func updateLive() {
        switch current {
        case .job, .search, .addJob, .addSearch: break
        case .connection: updateConnection(); return
        case .known: knownTable.reloadData(); return
        case .group: return
        }
        sidebar.reloadData()
        if let i = items.firstIndex(of: current) {
            sidebar.selectRowIndexes(IndexSet(integer: i), byExtendingSelection: false)
        }
        guard let d = liveData, !isNew else {
            editorStatus.stringValue = isSearch
                ? "New search — choose what to search by, Save, then Run. The results become their own tab in the Jira window."
                : "New poll job — give it a name, a schedule and a query, then Save. It becomes its own tab in the Jira window."
            editorStatus.textColor = .secondaryLabelColor
            requestText.string = "Save to see the exact JQL and curl requests."
            return
        }
        let name = d["name"] as? String ?? ""
        var st = d["status"] as? String ?? ""
        if !isSearch, JiraPoll.running.contains(name) || JiraPoll.running.contains("*") { st = "running" }
        if isSearch, JiraPoll.running.contains("search:\(name)") { st = "running" }
        var parts = ["● \(st)"]
        parts.append("last run \(JiraPoll.short(d["lastRun"] as? String))")
        if !isSearch {
            parts.append("next \(JiraPoll.short(d["nextRun"] as? String))")
            if (d["type"] as? String) == "issues" { parts.append("next window ≥ \(d["nextWindow"] as? String ?? "?")") }
        }
        if let n = d["items"] as? Int { parts.append("\(n) items") }
        editorStatus.stringValue = parts.joined(separator: "  ·  ")
        editorStatus.textColor = st == "ok" ? .systemGreen : st == "error" ? .systemRed
            : st == "running" ? .systemOrange : .secondaryLabelColor

        let out = NSMutableAttributedString()
        let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
        let bold = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .bold)
        func head(_ s: String) {
            out.append(NSAttributedString(string: s + "\n", attributes: [.font: bold, .foregroundColor: NSColor.secondaryLabelColor]))
        }
        func line(_ s: String, _ c: NSColor = .labelColor) {
            out.append(NSAttributedString(string: s + "\n", attributes: [.font: mono, .foregroundColor: c]))
        }
        if let jql = d["jql"] as? String, !jql.isEmpty {
            head("JQL\(isSearch ? "" : " (next run)")")
            line(jql)
        }
        for r in d["requests"] as? [[String: Any]] ?? [] {
            line("# \(r["purpose"] as? String ?? "")", .secondaryLabelColor)
            line(r["curl"] as? String ?? "")
        }
        if let err = d["lastError"] as? String, !err.isEmpty {
            head("\nLAST ERROR")
            line(err, .systemRed)
            if let c = d["lastCurl"] as? String, !c.isEmpty {
                line("# the failing request ($JIRA_TOKEN = your token)", .secondaryLabelColor)
                line(c)
            }
        }
        for n in d["notes"] as? [String] ?? [] { line("ℹ︎ \(n)", .secondaryLabelColor) }
        let keep = requestText.enclosingScrollView?.contentView.bounds.origin
        requestText.textStorage?.setAttributedString(out)
        if let o = keep { requestText.enclosingScrollView?.contentView.scroll(to: o) }
    }

    private func updateButtons() {
        saveButton.isEnabled = dirty || isNew
        revertButton.isEnabled = dirty && !isNew
        copyCurlButton.isEnabled = !isNew
        actionButton.isEnabled = !isNew
        if case .job(let n) = current, JiraPoll.running.contains(n) { actionButton.isEnabled = false }
        if case .search(let n) = current, JiraPoll.running.contains("search:\(n)") { actionButton.isEnabled = false }
        stopButton.isHidden = !(lockHeld || !JiraPoll.running.isEmpty)
    }

    private func markDirty() {
        dirty = true
        editorMsg.stringValue = ""
        updateButtons()
    }

    @objc private func textDidChange(_ n: Notification) {
        if (n.object as? NSTextField) === nameField { updateFormVisibility() }
        markDirty()
    }
    @objc private func fieldChanged(_ sender: Any?) { markDirty() }
    @objc private func popupChanged(_ sender: Any?) {
        updateFormVisibility()
        markDirty()
    }

    // MARK: save / delete / run

    private func parseArgs(_ s: String) -> [String: String] {
        var out: [String: String] = [:]
        for part in s.split(separator: ",") {
            let kv = part.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
            if kv.count == 2, !kv[0].isEmpty { out[kv[0]] = kv[1] }
        }
        return out
    }

    private func draftJSON() -> [String: Any] {
        window.makeFirstResponder(nil)   // commit an in-progress cell edit
        let name = nameField.stringValue.trimmingCharacters(in: .whitespaces)
        let pr = projectsField.stringValue.trimmingCharacters(in: .whitespaces)
        let plist = pr.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        var o: [String: Any] = ["name": name, "columns": ListColumn.serialize(colEditor.cols)]
        if isSearch {
            let kind = selectedKind["kind"] as? String ?? "text"
            let argName = selectedKind["arg"] as? String ?? "query"
            var args: [String: Any] = kind == "job" ? parseArgs(argsField.stringValue) : [:]
            args[argName] = valueField.stringValue.trimmingCharacters(in: .whitespaces)
            o["kind"] = kind
            o["args"] = args
            o["projects"] = plist
        } else {
            o["type"] = typePopup.titleOfSelectedItem ?? "issues"
            o["window"] = everyBox.stringValue.trimmingCharacters(in: .whitespaces)
            o["enabled"] = enabledCheck.state == .on
            o["projects"] = (pr.isEmpty || pr == "*") ? "*" as Any : plist as Any
            let q = queryPopup.indexOfSelectedItem
            o["jql"] = q == 1 ? jqlField.stringValue.trimmingCharacters(in: .whitespaces) : ""
            o["job"] = q >= 2 ? String((queryPopup.titleOfSelectedItem ?? "").dropFirst("team.json: ".count)) : ""
            o["args"] = q >= 2 ? parseArgs(argsField.stringValue) : [String: String]()
        }
        return o
    }

    @objc private func save(_ sender: Any?) {
        guard dirty || isNew else { return }
        switch current {
        case .job, .search, .addJob, .addSearch: persist(then: nil)
        default: return
        }
    }

    // validate + write via jira_config.py; `then` runs after a successful save
    private func persist(then: ((String) -> Void)?) {
        let o = draftJSON()
        guard let data = try? JSONSerialization.data(withJSONObject: o) else { return }
        let name = o["name"] as? String ?? ""
        let search = isSearch
        editorMsg.textColor = .secondaryLabelColor
        editorMsg.stringValue = "Saving…"
        JiraPoll.run("jira_config.py", [search ? "--upsert-search" : "--upsert-endpoint"],
                     stdin: String(decoding: data, as: UTF8.self)) { [weak self] code, out, err in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            guard code == 0, r["ok"] as? Bool == true else {
                let probs = r["problems"] as? [String] ?? [JiraPoll.errorLine(err, fallback: "save failed (exit \(code))")]
                self.editorMsg.textColor = .systemRed
                self.editorMsg.stringValue = "✗ " + probs.joined(separator: "\n✗ ")
                return
            }
            self.controller?.log("jira: saved \(search ? "search" : "job") \(name)")
            self.dirty = false
            self.isNew = false
            self.current = search ? .search(name) : .job(name)
            self.nameField.isEditable = false
            self.deleteButton.isHidden = false
            self.updateButtons()
            self.controller?.reloadJiraWindow()
            self.refresh {
                self.editorMsg.textColor = .systemGreen
                self.editorMsg.stringValue = "✓ Saved to config.json"
                then?(name)
            }
        }
    }

    @objc private func revert(_ sender: Any?) {
        dirty = false
        showItem(current)
    }

    @objc private func deleteItem(_ sender: Any?) {
        guard let name = editingName, let d = liveData else { return }
        let a = NSAlert()
        a.alertStyle = .warning
        a.messageText = "Delete \(isSearch ? "search" : "poll job") “\(name)”?"
        a.informativeText = "It is removed from config.json and its tab (\(d["file"] as? String ?? "")) "
            + "disappears from the Jira window."
        a.addButton(withTitle: "Delete")
        a.addButton(withTitle: "Cancel")
        ask(a) { [weak self] ok in if ok { self?.performDelete(name, d) } }
    }

    private func performDelete(_ name: String, _ d: [String: Any]) {
        let search = isSearch
        let path = tabPath(d)
        JiraPoll.run("jira_config.py", [search ? "--delete-search" : "--delete-endpoint", name]) { [weak self] code, out, _ in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            guard code == 0, r["ok"] as? Bool == true else {
                self.editorMsg.textColor = .systemRed
                self.editorMsg.stringValue = "✗ " + (r["problems"] as? [String] ?? ["delete failed"]).joined(separator: "; ")
                return
            }
            // the tab file goes with it (otherwise it lingers as a stale tab)
            if let path { try? FileManager.default.removeItem(atPath: path) }
            self.controller?.log("jira: deleted \(search ? "search" : "job") \(name)")
            self.dirty = false
            self.didInitialSelect = false
            self.controller?.reloadJiraWindow()
            self.refresh()
        }
    }

    // absolute path of an item's tab file (outDir is the endpoints' dir)
    private func tabPath(_ d: [String: Any]) -> String? {
        if let p = d["path"] as? String, !p.isEmpty { return p }
        guard let f = d["file"] as? String, !f.isEmpty,
              let p = eps.first?["path"] as? String else { return nil }
        return ((p as NSString).deletingLastPathComponent as NSString).appendingPathComponent(f)
    }

    @objc private func primaryAction(_ sender: Any?) {
        if dirty {
            // Force Poll / Run act on the SAVED definition: save first
            persist { [weak self] _ in self?.primaryAction(nil) }
            return
        }
        guard let name = editingName else { return }
        if isSearch { runSearch(name); return }
        guard lockHeld || !JiraPoll.running.isEmpty else { forcePoll(name); return }
        let lock = info["lock"] as? [String: Any] ?? [:]
        let a = NSAlert()
        a.alertStyle = .warning
        a.messageText = "A poll is already running"
        a.informativeText = "pid \(lock["pid"] ?? "?") since \(lock["since"] as? String ?? "?")"
            + (JiraPoll.running.isEmpty ? "" : " (\(JiraPoll.running.sorted().joined(separator: ", ")))")
            + ".\n\nStop it and poll “\(name)” now?"
        a.addButton(withTitle: "Stop It and Poll Now")
        a.addButton(withTitle: "Cancel")
        ask(a) { [weak self] ok in
            guard ok else { return }
            JiraPoll.run("jira_poll.py", ["--cancel"]) { [weak self] _, _, _ in
                // give the stopped poll a moment to release the lock
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { self?.forcePoll(name) }
            }
        }
    }

    private func forcePoll(_ name: String) {
        controller?.jiraPollNow(name) { [weak self] in
            self?.controller?.reloadJiraWindow()
            self?.refresh()
        }
        updateButtons()
        updateLive()
        refresh()
    }

    private func runSearch(_ name: String) {
        let tag = "search:\(name)"
        guard !JiraPoll.running.contains(tag) else { return }
        JiraPoll.running.insert(tag)
        updateButtons()
        updateLive()
        editorMsg.textColor = .secondaryLabelColor
        editorMsg.stringValue = "Running…"
        JiraPoll.run("jira_poll.py", ["--search", name, "--quiet"]) { [weak self] code, _, err in
            JiraPoll.running.remove(tag)
            guard let self else { return }
            if code == 0 {
                self.editorMsg.textColor = .systemGreen
                self.editorMsg.stringValue = "✓ Done — see its tab in the Jira window"
                self.controller?.reloadJiraWindow()
            } else {
                self.editorMsg.textColor = .systemRed
                self.editorMsg.stringValue = "✗ " + JiraPoll.errorLine(err, fallback: "search failed (exit \(code))")
            }
            self.updateButtons()
            self.refresh()
        }
    }

    @objc private func copyCurl(_ sender: Any?) {
        let reqs = liveData?["requests"] as? [[String: Any]] ?? []
        let text = reqs.map { "# \($0["purpose"] as? String ?? "")\n\($0["curl"] as? String ?? "")" }
            .joined(separator: "\n")
        guard !text.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        editorMsg.textColor = .secondaryLabelColor
        editorMsg.stringValue = "curl copied (\(reqs.count) request\(reqs.count == 1 ? "" : "s"), includes the token)"
    }

    // MARK: header actions

    @objc private func toggleEnabled(_ sender: Any?) {
        controller?.toggleJiraPoll()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.refresh() }
    }

    @objc private func setup(_ sender: Any?) { controller?.showJiraSetup() }

    @objc private func stopPoll(_ sender: Any?) {
        JiraPoll.run("jira_poll.py", ["--cancel"]) { [weak self] code, out, err in
            self?.controller?.log("jira: stop -> \(out.trimmingCharacters(in: .whitespacesAndNewlines))"
                                  + (code == 0 ? "" : " " + err))
            self?.refresh()
        }
    }

    @objc private func openFile(_ sender: NSPopUpButton) {
        guard let path = sender.selectedItem?.representedObject as? String else { return }
        let fm = FileManager.default
        if !fm.fileExists(atPath: path) {
            // team.json: start from the shipped example
            let example = JiraPoll.dir + "/team.example.json"
            try? fm.createDirectory(atPath: (path as NSString).deletingLastPathComponent,
                                    withIntermediateDirectories: true)
            guard (try? fm.copyItem(atPath: example, toPath: path)) != nil else { return }
        }
        controller?.openNoteFile(path)
        refresh()
    }

    // MARK: connection

    private func showConnection() {
        let buttons = row([
            button(NSButton(title: "Test Connection", target: nil, action: nil), #selector(testConnection(_:)),
                   tip: "GET /rest/api/2/myself with the saved config"),
            button(NSButton(title: "Copy Login curl", target: nil, action: nil), #selector(copyLoginCurl(_:))),
            button(NSButton(title: "Setup…", target: nil, action: nil), #selector(setup(_:))),
        ])
        connResult.font = .systemFont(ofSize: 12)
        connResult.removeFromSuperview()
        let sv = monoTextView(connText)
        sv.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        let page = vstack([sectionTitle("CONNECTION"), buttons, connResult, sv], spacing: 8)
        for v in [connResult, sv] as [NSView] { v.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true }
        showPage(page)
        updateConnection()
    }

    private func updateConnection() {
        let s = { (k: String) in self.info[k] as? String ?? "" }
        let auth = s("auth")
        var t = """
        site        \(s("site"))
        auth        \(auth) — \(auth == "basic" ? "Cloud: email + API token (curl -u)" : "Server/Data Center: personal access token (Authorization: Bearer)")
        token       \((info["hasToken"] as? Bool ?? false) ? "set" : "MISSING")
        config      \(s("configPath"))
        team.json   \(s("teamPath"))\((info["teamExists"] as? Bool ?? false) ? "" : "  (not created — Open… ▸ Team schema)")
        status      \(s("statusPath"))
        curl log    \(s("curlLog"))
        poller      \(s("pollScript"))

        LOGIN TEST (GET /myself) — full curl:
        \(s("loginCurl"))
        """
        if let pk = info["projectKeys"] as? [String], !pk.isEmpty {
            t += "\n\nproject_keys  \(pk.joined(separator: ", "))"
        }
        connText.string = t
    }

    @objc private func testConnection(_ sender: Any?) {
        connResult.stringValue = "Testing…"
        connResult.textColor = .secondaryLabelColor
        JiraPoll.run("jira_api.py", ["--myself", "--no-auth-check"]) { [weak self] code, out, err in
            guard let self else { return }
            if code == 0, let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] {
                self.connResult.stringValue = "✓ Connected as \(d["displayName"] as? String ?? "unknown user")"
                self.connResult.textColor = .systemGreen
            } else {
                self.connResult.stringValue = "✗ " + JiraPoll.errorLine(err, fallback: "login failed (exit \(code))")
                self.connResult.textColor = .systemRed
            }
        }
    }

    @objc private func copyLoginCurl(_ sender: Any?) {
        guard let c = info["loginCurl"] as? String, !c.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(c, forType: .string)
        connResult.stringValue = "Login curl copied (includes the token)."
        connResult.textColor = .secondaryLabelColor
    }

    // MARK: known columns

    private static let knownSpec: [(id: String, title: String, width: CGFloat)] = [
        ("field", "Field", 150), ("titles", "Title(s)", 130), ("usedBy", "Used by", 200),
        ("seenIn", "Has data in", 180), ("api", "Fetches (API field)", 150), ("label", "Custom field label", 150),
    ]

    private func showKnown() {
        if knownTable.tableColumns.isEmpty {
            for c in Self.knownSpec {
                let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(c.id))
                col.title = c.title
                col.width = c.width
                knownTable.addTableColumn(col)
            }
            knownTable.dataSource = self
            knownTable.delegate = self
            knownTable.usesAlternatingRowBackgroundColors = true
            knownTable.rowHeight = 22
        }
        knownTable.enclosingScrollView?.removeFromSuperview()
        let sv = NSScrollView()
        sv.documentView = knownTable
        sv.hasVerticalScroller = true
        sv.hasHorizontalScroller = true
        sv.borderType = .bezelBorder
        sv.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        let hint = NSTextField(wrappingLabelWithString:
            "Every column any poll job or search defines, every field that came back with data "
            + "(fields_seen.json), plus base fields and team.json custom fields — the building blocks "
            + "for a new job's or search's columns (its column editor's Add list).")
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = .secondaryLabelColor
        let page = vstack([sectionTitle("KNOWN COLUMNS"), hint, sv], spacing: 8)
        for v in [hint, sv] as [NSView] { v.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true }
        showPage(page)
        knownTable.reloadData()
    }

    private func knownCell(_ id: String, _ row: Int) -> NSView? {
        guard row < catalog.count else { return nil }
        let c = catalog[row]
        let s: String
        switch id {
        case "field": s = c["field"] as? String ?? ""
        case "titles": s = (c["titles"] as? [String] ?? []).joined(separator: ", ")
        case "usedBy": s = (c["usedBy"] as? [String] ?? []).joined(separator: ", ")
        case "seenIn": s = (c["seenIn"] as? [String] ?? []).joined(separator: ", ")
        case "api": s = (c["apiFields"] as? [String] ?? []).joined(separator: ", ")
        case "label": s = c["label"] as? String ?? ""
        default: s = ""
        }
        let l = NSTextField(labelWithString: s)
        l.lineBreakMode = .byTruncatingTail
        l.toolTip = s
        if id == "field" || id == "api" { l.font = .monospacedSystemFont(ofSize: 11, weight: .regular) }
        if id != "field" { l.textColor = .secondaryLabelColor }
        let cell = NSTableCellView()
        l.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(l)
        NSLayoutConstraint.activate([
            l.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 3),
            l.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -3),
            l.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    // MARK: close

    private func close() {
        guard window.attachedSheet == nil else { return }
        confirmDiscard { [weak self] in
            self?.window.orderOut(nil)
            self?.teardown()
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard dirty else { return true }
        confirmDiscard { [weak self] in self?.window.close() }
        return false
    }

    private func teardown() {
        timer?.invalidate()
        timer = nil
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        NotificationCenter.default.removeObserver(self)
        JiraDashboardWindow.live = nil
    }

    func windowWillClose(_ notification: Notification) { teardown() }
}
