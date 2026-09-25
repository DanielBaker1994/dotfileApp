import AppKit

// MARK: - Jira Config window (the one place for everything jira-poll)
//
// Menu bar "Open Jira Config Window" (also the Jira window's icon menu, and
// `workspace-switcher jira-poll dashboard`). Master–detail:
//
//   sidebar          POLL JOBS  (each endpoint in config.json, + Add Poll Job)
//                    SETTINGS   (Live Search, Connection, Definitions)
//   detail           the selected item's editor
//
// Poll job editor: its settings (projects from a picker, page size / max
// issues), ITS OWN columns (each job writes one tab of the Jira window and
// owns that tab's table), the full JQL and the full curl of every request
// (Copy curl), Save / Revert / Delete, and Force Poll (warns when a poll is
// already running). The `directory` job caches projects + users + statuses
// for the pickers (weekly; no tab).
// Live Search: the Cmd+F search tab's columns + max results.
// Definitions: everything team.json defines (projects, custom fields, API
// endpoints, JQL templates, search defaults) — add / edit (double-click) /
// remove — plus read-only views of the cached users / statuses / columns.
//
// Everything shown comes from ONE python call — `jira_poll.py --describe`
// (+ directory.json) — and every edit goes through jira_config.py
// (--upsert-endpoint / --delete-endpoint / --set-columns / --set-live-search
// / --team-set), which validates before writing. Config stays the source of
// truth; the window is a view + editor over it.
// Sizes / refresh: [jira] dashboard-width, dashboard-height, dashboard-refresh.

// A label/control form in an NSAlert sheet (the window floats above the
// popups: an app-modal alert would open hidden behind it). `then(true)` =
// the first button.
func jiraFormSheet(on window: NSWindow, title: String, info: String, rows: [(String, NSView)],
                   ok: String = "Save", first: NSView? = nil, then: @escaping (Bool) -> Void) {
    let g = NSGridView(views: rows.map { r -> [NSView] in
        let l = NSTextField(labelWithString: r.0)
        l.alignment = .right
        l.textColor = .secondaryLabelColor
        return [l, r.1]
    })
    g.rowSpacing = 8
    g.columnSpacing = 10
    g.column(at: 0).xPlacement = .trailing
    g.rowAlignment = .firstBaseline
    for (_, v) in rows where v is NSTextField || v is JiraMultiPicker {
        v.widthAnchor.constraint(greaterThanOrEqualToConstant: 340).isActive = true
    }
    g.layoutSubtreeIfNeeded()
    g.setFrameSize(NSSize(width: max(460, g.fittingSize.width), height: g.fittingSize.height))
    let a = NSAlert()
    a.messageText = title
    a.informativeText = info
    a.accessoryView = g
    a.addButton(withTitle: ok)
    a.addButton(withTitle: "Cancel")
    a.window.initialFirstResponder = first ?? rows.first?.1
    a.beginSheetModal(for: window) { r in then(r == .alertFirstButtonReturn) }
}

// One column list editor (a job's or the live search's columns). Rows are
// read-only; double-click (or Edit…, or Return) opens the column's sheet.
final class JiraColumnEditor: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    var cols: [ListColumn] = [] { didSet { table.reloadData() } }
    var meta: [String: [String: Any]] = [:]      // field -> catalog entry (apiFields, label)
    var catalogFields: [String] = []
    var onChange: (() -> Void)?
    weak var sheetWindow: NSWindow?
    let table = NSTableView()
    let copyFrom = NSPopUpButton(frame: .zero, pullsDown: true)
    var copySources: [(String, String)] = []     // (menu title, columns spec)
    private(set) var view = NSView()

    private static let spec: [(id: String, title: String, width: CGFloat)] = [
        ("title", "Header (field label)", 150), ("field", "Field (what is fetched)", 260), ("width", "Width %", 62),
        ("align", "Align", 58), ("sort", "Sort", 40), ("filter", "Filter", 44),
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
        table.rowHeight = 22
        table.usesAlternatingRowBackgroundColors = true
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.target = self
        table.doubleAction = #selector(editClicked(_:))
        let sv = NSScrollView()
        sv.documentView = table
        sv.hasVerticalScroller = true
        sv.hasHorizontalScroller = true
        sv.borderType = .bezelBorder

        copyFrom.addItem(withTitle: "Copy columns from…")
        copyFrom.target = self
        copyFrom.action = #selector(copyColumns(_:))
        copyFrom.controlSize = .small
        func btn(_ t: String, _ a: Selector, _ tip: String? = nil) -> NSButton {
            let b = NSButton(title: t, target: self, action: a)
            b.bezelStyle = .rounded
            b.controlSize = .small
            b.toolTip = tip
            return b
        }
        let hint = NSTextField(labelWithString: "Double-click a column to edit it")
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = .tertiaryLabelColor
        let bar = NSStackView(views: [btn("Add…", #selector(add(_:))), btn("Edit…", #selector(editClicked(_:))),
                                      btn("Remove", #selector(remove(_:))),
                                      btn("◀", #selector(up(_:)), "Move left"),
                                      btn("▶", #selector(down(_:)), "Move right"), copyFrom, hint])
        bar.orientation = .horizontal
        bar.spacing = 6
        bar.setHuggingPriority(.required, for: .vertical)
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
        catalogFields = fields
        copySources = sources
        while copyFrom.numberOfItems > 1 { copyFrom.removeItem(at: 1) }
        for (t, _) in sources { copyFrom.addItem(withTitle: t) }
    }

    private func changed() { table.reloadData(); onChange?() }

    // the field's ONE label (Definitions ▸ Fields) + the Jira field(s) it fetches
    func fieldName(_ f: String) -> String {
        let m = meta[f] ?? [:]
        return (m["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? JiraPoll.baseFieldLabels[f] ?? f
    }
    func apiText(_ f: String) -> String {
        let api = (meta[f]?["apiFields"] as? [String]).map { $0.isEmpty ? "(no API field)" : $0.joined(separator: ", ") } ?? f
        return api == f ? "" : api
    }

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

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let id = tableColumn?.identifier.rawValue, row < cols.count else { return nil }
        let c = cols[row]
        let l = NSTextField(labelWithString: "")
        l.lineBreakMode = .byTruncatingTail
        switch id {
        case "title": l.stringValue = fieldName(c.field)
        case "field":
            let s = NSMutableAttributedString(string: c.field, attributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular), .foregroundColor: NSColor.labelColor])
            let extra = [apiText(c.field)].filter { !$0.isEmpty && $0 != c.field }
            if !extra.isEmpty {
                s.append(NSAttributedString(string: "  " + extra.joined(separator: " · "), attributes: [
                    .font: NSFont.systemFont(ofSize: 11), .foregroundColor: NSColor.secondaryLabelColor]))
            }
            l.attributedStringValue = s
        case "width": l.stringValue = c.width == 0 ? "auto" : (c.width == c.width.rounded() ? String(Int(c.width)) : String(format: "%.1f", c.width))
        case "align": l.stringValue = c.align
        case "sort": l.stringValue = c.sortable ? "✓" : ""
        case "filter": l.stringValue = c.filterable ? "✓" : ""
        default: return nil
        }
        if id == "width" || id == "align" { l.textColor = .secondaryLabelColor }
        l.toolTip = id == "field" ? "\(c.field) — \(fieldName(c.field)) \(apiText(c.field))" : l.stringValue
        return cell(l)
    }

    // MARK: edit sheet

    @objc func editClicked(_ sender: Any?) {
        let r = sender is NSTableView ? table.clickedRow : table.selectedRow
        guard r >= 0, r < cols.count else { return }
        editSheet(r)
    }
    func editSelected() { editClicked(nil) }

    @objc func add(_ sender: Any?) { editSheet(nil) }

    private func editSheet(_ index: Int?) {
        guard let win = sheetWindow else { return }
        let c = index.map { cols[$0] }
            ?? ListColumn(field: "", title: "", width: 0, align: "left", sortable: true, filterable: true)
        let field = NSComboBox()
        field.addItems(withObjectValues: catalogFields.filter { f in f == c.field || !cols.contains { $0.field == f } })
        field.completes = true
        field.numberOfVisibleItems = 14
        field.stringValue = c.field
        field.placeholderString = "created, duedate, customfield_10010, a team alias…"
        let width = NSTextField(string: c.width == 0 ? "" : String(format: c.width == c.width.rounded() ? "%.0f" : "%.1f", c.width))
        width.placeholderString = "percent of the row — empty = share the leftover"
        let align = NSPopUpButton(frame: .zero, pullsDown: false)
        align.addItems(withTitles: ["left", "center", "right"])
        align.selectItem(withTitle: c.align)
        let sort = NSButton(checkboxWithTitle: "Sortable — click the header to sort", target: nil, action: nil)
        sort.state = c.sortable ? .on : .off
        let filter = NSButton(checkboxWithTitle: "Filterable — a dropdown of its values in the window", target: nil, action: nil)
        filter.state = c.filterable ? .on : .off
        let info = "Field = what the poll fetches from Jira and shows (a window field like title, a "
            + "team.json custom field alias, or a raw Jira field id). The header is the field's label — "
            + "rename it once in Definitions ▸ Fields and every job and the search tab follow."
        jiraFormSheet(on: win, title: index == nil ? "Add Column" : "Edit Column “\(fieldName(c.field))”", info: info,
                      rows: [("Field", field), ("Width %", width), ("Align", align),
                             ("", sort), ("", filter)],
                      first: index == nil ? field : width) { [weak self] ok in
            guard ok, let self else { return }
            // ':' and ',' are the columns-line separators
            func clean(_ s: String) -> String {
                s.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ":", with: " ")
                    .replacingOccurrences(of: ",", with: " ")
            }
            let f = clean(field.stringValue).replacingOccurrences(of: " ", with: "")
            guard !f.isEmpty else { NSSound.beep(); return }
            if self.cols.enumerated().contains(where: { $0.offset != index && $0.element.field == f }) {
                NSSound.beep()
                return
            }
            var nc = c
            nc.field = f
            nc.title = ""
            nc.width = CGFloat(min(100, max(0, Double(width.stringValue.trimmingCharacters(in: .whitespaces)) ?? 0)))
            nc.align = align.titleOfSelectedItem ?? "left"
            nc.sortable = sort.state == .on
            nc.filterable = filter.state == .on
            let at: Int
            if let i = index { self.cols[i] = nc; at = i } else { self.cols.append(nc); at = self.cols.count - 1 }
            self.changed()
            self.table.selectRowIndexes(IndexSet(integer: at), byExtendingSelection: false)
            self.table.scrollRowToVisible(at)
        }
    }

    @objc func remove(_ sender: Any?) {
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

// The Jira Config window wears the popup windows' look: titled (sheets,
// native resize) but with the titlebar hidden, the card's blur + tint +
// border, and the SAME header strip (✕ close · app icon · title). The
// invisible titlebar swallows header clicks, so they're caught here (as in
// PopupBaseWindow); drags stay native.
final class JiraConfigNSWindow: NSWindow {
    var cornerRadius: CGFloat = 10
    @objc func _cornerRadius() -> CGFloat { cornerRadius }
    var headerBand: CGFloat = 0
    var onHeaderClick: ((NSPoint) -> Void)?    // flipped (top-down) window coords
    private var down: NSPoint?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
    override func sendEvent(_ event: NSEvent) {
        if headerBand > 0 {
            switch event.type {
            case .leftMouseDown where event.locationInWindow.y >= frame.height - headerBand:
                down = NSEvent.mouseLocation
            case .leftMouseUp:
                if let d = down {
                    down = nil
                    let m = NSEvent.mouseLocation
                    if hypot(m.x - d.x, m.y - d.y) < 4 {
                        let l = event.locationInWindow
                        onHeaderClick?(NSPoint(x: l.x, y: frame.height - l.y))
                    }
                }
            default: break
            }
        }
        super.sendEvent(event)
    }
}

final class JiraDashboardWindow: NSObject, NSWindowDelegate, NSTableViewDataSource,
                                 NSTableViewDelegate {
    private static var live: JiraDashboardWindow?

    private enum Item: Equatable {
        case group(String), job(String), addJob, liveSearch, connection, definitions
    }

    private weak var controller: SwitcherController?
    private let window: JiraConfigNSWindow
    private var chrome: PopupChrome?
    private var monitor: Any?
    private var timer: Timer?
    private var describing = false

    // data (jira_poll.py --describe + directory.json)
    private var info: [String: Any] = [:]
    private var eps: [[String: Any]] = []
    private var catalog: [[String: Any]] = []
    private var dir = JiraDirectory()
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

    // job editor state
    private var isNew = false
    private var dirty = false
    private let colEditor = JiraColumnEditor()
    private let nameField = NSTextField()
    private let fileLabel = NSTextField(labelWithString: "")
    // where the job lives: config.json › endpoints › NAME (+ open it)
    private let sourceLabel = NSTextField(labelWithString: "")
    private let openConfigButton = NSButton(title: "Open config.json", target: nil, action: nil)
    private lazy var sourceRow: NSStackView = row([sourceLabel, openConfigButton])
    private let typePopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let everyBox = NSComboBox()
    private let projectsPicker = JiraMultiPicker(noun: "project", allTitle: "All projects")
    private let queryPopup = NSPopUpButton(frame: .zero, pullsDown: false)
    private let jqlField = NSTextField()
    private let argsField = NSTextField()
    private let pageSizeField = NSTextField()
    private let maxTotalField = NSTextField()
    private let enabledCheck = NSButton(checkboxWithTitle: "Scheduled (poll on its interval)", target: nil, action: nil)
    private let editorStatus = NSTextField(wrappingLabelWithString: "")
    private let editorMsg = NSTextField(wrappingLabelWithString: "")
    private let requestText = NSTextView()
    private let saveButton = NSButton(title: "Save", target: nil, action: nil)
    private let revertButton = NSButton(title: "Revert", target: nil, action: nil)
    private let deleteButton = NSButton(title: "Delete…", target: nil, action: nil)
    private let actionButton = NSButton(title: "Force Poll", target: nil, action: nil)
    private let copyCurlButton = NSButton(title: "Copy curl", target: nil, action: nil)
    private var colsTitle = NSTextField(labelWithString: "")
    private let liveMaxField = NSTextField()

    // connection / definitions
    private let connText = NSTextView()
    private let connResult = NSTextField(wrappingLabelWithString: "")
    private enum DefTab: String, CaseIterable {
        case projects = "Projects", fields = "Fields", users = "Users",
             lists = "Statuses & Types", api = "API Endpoints", jql = "JQL Templates", defaults = "Search Defaults"
        var editable: Bool { ![.users, .lists].contains(self) }
    }
    private var defTab: DefTab = .projects
    private let defSeg = NSSegmentedControl()
    private let defTable = NSTableView()
    private var defRows: [[String]] = []
    private var defKeys: [String] = []           // row -> team.json key / alias / project key
    private let defHint = NSTextField(wrappingLabelWithString: "")
    private let defMsg = NSTextField(wrappingLabelWithString: "")
    private let defAdd = NSButton(title: "Add…", target: nil, action: nil)
    private let defEdit = NSButton(title: "Edit…", target: nil, action: nil)
    private let defRemove = NSButton(title: "Remove", target: nil, action: nil)
    private let defFetch = NSButton(title: "Fetch from Jira now", target: nil, action: nil)

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
        window = JiraConfigNSWindow(contentRect: NSRect(x: 0, y: 0, width: W, height: H),
                                    styleMask: [.titled, .closable, .resizable, .miniaturizable,
                                                .fullSizeContentView],
                                    backing: .buffered, defer: false)
        super.init()
        window.title = "Jira Config"
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 860, height: 520)
        // same level as the setup window: above the popup windows, which
        // float at .popUpMenu
        window.level = NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 1)
        window.delegate = self
        window.contentView = themedRoot(buildContent())
        colEditor.onChange = { [weak self] in self?.markDirty() }
        colEditor.sheetWindow = window
        installKeys()
    }

    // the popup windows' surface around `content`: blur + card tint + border,
    // rounded like them, with their header strip on top
    private func themedRoot(_ content: NSView) -> NSView {
        var cfg = PopupConfig(name: "jira-config")
        cfg.colors = PopupColors(background: BAR, border: BORDER, text: TEXT, dim: DIM,
                                 highlight: GROUP_BG, accent: ACCENT)
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = headerBlueSilver
        let radius = cfg.cornerRadius + 1
        window.cornerRadius = radius
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        for b: NSWindow.ButtonType in [.closeButton, .miniaturizeButton, .zoomButton] {
            window.standardWindowButton(b)?.isHidden = true
        }
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = true
        let light = ButtonStyle.luminance(BAR) > 0.45
        window.appearance = NSAppearance(named: light ? .aqua : .darkAqua)

        let root = NSView()
        root.wantsLayer = true
        root.layer?.cornerRadius = radius
        root.layer?.masksToBounds = true
        let fx = NSVisualEffectView()
        fx.material = cfg.material
        fx.blendingMode = .behindWindow
        fx.state = .active
        let tint = NSView()
        tint.wantsLayer = true
        // denser than the list windows' card: this window is all form text
        tint.layer?.backgroundColor = BAR.withAlphaComponent(max(cfg.tintAlpha, 0.92)).cgColor
        tint.layer?.borderColor = BORDER.cgColor
        tint.layer?.borderWidth = 1
        tint.layer?.cornerRadius = radius
        let ch = PopupChrome(config: cfg)
        ch.dragHeaderHeight = cfg.headerHeight
        ch.headerIcon = jiraAppIcon
        ch.headerTitle = "Jira Config"
        ch.copyPathLabel = ""
        ch.copyConfigLabel = ""
        chrome = ch
        for v in [fx, tint, ch, content] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        for v in [fx, tint] as [NSView] { pinned(v, in: root) }
        NSLayoutConstraint.activate([
            ch.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            ch.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            ch.topAnchor.constraint(equalTo: root.topAnchor),
            ch.heightAnchor.constraint(equalToConstant: cfg.headerHeight),
            content.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 1),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -1),
            content.topAnchor.constraint(equalTo: ch.bottomAnchor),
            content.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -1),
        ])
        window.headerBand = cfg.headerHeight
        window.onHeaderClick = { [weak self] p in
            guard let self, let ch = self.chrome else { return }
            if ch.closeButtonRect.insetBy(dx: -2, dy: -2).contains(p) { self.close() }
        }
        return root
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

    private func hint(_ s: String) -> NSTextField {
        let l = NSTextField(wrappingLabelWithString: s)
        l.font = .systemFont(ofSize: 11)
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
        setupDefinitions()
        return content
    }

    private var configPath: String { info["configPath"] as? String ?? JiraPoll.configPath }
    private func tilde(_ p: String) -> String { (p as NSString).abbreviatingWithTildeInPath }

    @objc private func openConfigJSON(_ sender: Any?) {
        controller?.openNoteFile(configPath)
    }

    private func setupEditorControls() {
        sourceLabel.textColor = .secondaryLabelColor
        sourceLabel.lineBreakMode = .byTruncatingMiddle
        sourceLabel.isSelectable = true
        openConfigButton.controlSize = .small
        button(openConfigButton, #selector(openConfigJSON(_:)),
               tip: "Every poll job is one entry of \"endpoints\" in this file — this window edits it for you")
        typePopup.addItems(withTitles: ["issues", "releases", "directory"])
        everyBox.addItems(withObjectValues: JiraPoll.intervals)
        everyBox.completes = true
        jqlField.placeholderString = "extra JQL, ANDed with the time window (e.g. assignee = currentUser())"
        argsField.placeholderString = "name=value, name=value"
        maxTotalField.placeholderString = "empty = every matching issue"
        liveMaxField.placeholderString = "100"
        projectsPicker.placeholder = "All projects"
        projectsPicker.onChange = { [weak self] in self?.markDirty() }
        for f in [nameField, jqlField, argsField, everyBox, pageSizeField, maxTotalField, liveMaxField] as [NSTextField] {
            NotificationCenter.default.addObserver(self, selector: #selector(textDidChange(_:)),
                                                   name: NSControl.textDidChangeNotification, object: f)
        }
        everyBox.target = self
        everyBox.action = #selector(fieldChanged(_:))
        for p in [typePopup, queryPopup] {
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
            guard let self else { return e }
            // a sheet (column / definition editor) is its own key window
            if let sheet = self.window.attachedSheet {
                return sheet.isKeyWindow && JiraEditKeys.route(e, in: sheet) ? nil : e
            }
            guard self.window.isKeyWindow else { return e }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command)
            let fr = self.window.firstResponder
            let editing = (fr as? NSTextView)?.isEditable == true
            if e.keyCode == 53 {                       // Esc: end an edit, else close
                if editing { self.window.makeFirstResponder(nil); return nil }
                self.close()
                return nil
            }
            if cmd && e.keyCode == 13 { self.close(); return nil }           // Cmd+W
            if cmd && e.keyCode == 15 { self.refresh(); return nil }         // Cmd+R
            if cmd && e.keyCode == 1 { self.save(nil); return nil }          // Cmd+S
            // Return edits / Delete removes the selected column or definition
            if !cmd, fr === self.colEditor.table {
                if e.keyCode == 36 { self.colEditor.editSelected(); return nil }
                if e.keyCode == 51 { self.colEditor.remove(nil); return nil }
            }
            if !cmd, fr === self.defTable, self.defTab.editable {
                if e.keyCode == 36 { self.defEditClicked(nil); return nil }
                if e.keyCode == 51 { self.defRemoveClicked(nil); return nil }
            }
            return JiraEditKeys.route(e, in: self.window) ? nil : e
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
        catalog = d["catalog"] as? [[String: Any]] ?? []
        let fetched = (d["directory"] as? [String: Any])?["fetchedAt"] as? String ?? ""
        if fetched != dir.fetchedAt { dir = JiraDirectory.load() }
        var its: [Item] = [.group("POLL JOBS")]
        its += eps.compactMap { ($0["name"] as? String).map(Item.job) }
        its += [.addJob, .group("SETTINGS"), .liveSearch, .connection, .definitions]
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
    private var searchDefault: Int { (info["searchDefaults"] as? [String: Any])?["max_results_search"] as? Int ?? 50 }

    private func updateHeader() {
        let enabled = info["enabled"] as? Bool ?? jiraEnabledInConfig()
        let bg = info["backgroundPoll"] as? Bool ?? false
        let lock = info["lock"] as? [String: Any] ?? [:]
        // one plain sentence; the plumbing (launchd tick, job count, lock pid)
        // lives in the tooltip
        let lr = info["lastRun"] as? String ?? ""
        let failed = !(info["lastError"] as? String ?? "").isEmpty
        var line = enabled ? "● Polling on" : bg ? "◐ Polling in the background (Jira window off)" : "○ Polling off"
        if lockHeld {
            line += " — polling now…"
        } else if !lr.isEmpty {
            line += failed ? " — last poll failed \(JiraPoll.short(lr))" : " — last checked \(JiraPoll.short(lr))"
        }
        statusLine.stringValue = line
        statusLine.textColor = !enabled ? .secondaryLabelColor : failed ? .systemOrange : .labelColor
        var tip = ["\(eps.count) poll job\(eps.count == 1 ? "" : "s") in \(info["configPath"] as? String ?? JiraPoll.configPath)",
                   "launchd tick: \(info["tick"] as? String ?? "60s") — each job runs when its own interval is due"]
        if !lr.isEmpty { tip.append("last run \(lr) \(info["status"] as? String ?? "")") }
        if lockHeld { tip.append("polling now: pid \(lock["pid"] ?? "?") since \(JiraPoll.short(lock["since"] as? String))") }
        statusLine.toolTip = tip.joined(separator: "\n")
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
            ("Directory cache (directory.json)", JiraPoll.directoryPath),
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
        tableView === sidebar ? items.count : defRows.count
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
        if tableView === defTable { return defCell(tableColumn?.identifier.rawValue ?? "", row) }
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
        case .addJob:
            l.stringValue = "+ Add Poll Job"
            l.textColor = .controlAccentColor
        case .liveSearch: l.stringValue = "Live Search  ⌘F"
        case .connection: l.stringValue = "Connection"
        case .definitions: l.stringValue = "Definitions"
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
        a.informativeText = current == .liveSearch ? "The live search settings are not saved."
            : "The edits to this poll job are not saved."
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
        case .job(let n): showEditor(data: eps.first { $0["name"] as? String == n }, new: false)
        case .addJob: showEditor(data: nil, new: true)
        case .liveSearch: showLiveSearch()
        case .connection: showConnection()
        case .definitions: showDefinitions()
        case .group: break
        }
    }

    private var editingName: String? {
        if case .job(let n) = current { return n }
        return nil
    }
    private var liveData: [String: Any]? {
        guard let n = editingName else { return nil }
        return eps.first { $0["name"] as? String == n }
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

    private func columnSources(excluding me: String?) -> [(String, String)] {
        var sources: [(String, String)] = [("[jira] columns (starter template)", info["columnsTemplate"] as? String ?? "")]
        for e in eps where (e["name"] as? String) != me && (e["type"] as? String) != "directory" {
            sources.append(("job: \(e["name"] as? String ?? "")", e["columnsSpec"] as? String ?? ""))
        }
        if me != "\u{0}live", let ls = info["liveSearch"] as? [String: Any] {
            sources.append(("live search", ls["columnsSpec"] as? String ?? ""))
        }
        return sources
    }

    private func loadColumns(_ spec: String?, me: String?) {
        var meta: [String: [String: Any]] = [:]
        for c in catalog { if let f = c["field"] as? String { meta[f] = c } }
        colEditor.meta = meta
        colEditor.cols = ListColumn.parse(spec ?? (info["columnsTemplate"] as? String ?? ""))
        colEditor.setCatalog(info["availableFields"] as? [String] ?? [], sources: columnSources(excluding: me))
    }

    private func showEditor(data: [String: Any]?, new: Bool) {
        isNew = new
        let d = data ?? [:]
        nameField.stringValue = d["name"] as? String ?? ""
        nameField.isEditable = new
        nameField.isSelectable = true
        nameField.placeholderString = "e.g. team-bugs"
        // projects: only known keys (directory ∪ team.json project_keys)
        projectsPicker.options = dir.projectOptions(extra: info["projectKeys"] as? [String] ?? [])
        if let p = d["projects"] as? [String] { projectsPicker.set(p) } else { projectsPicker.set([], all: true) }
        loadColumns(d["columnsSpec"] as? String, me: d["name"] as? String)

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
        let ps = d["maxResults"] as? Int ?? 0, mt = d["maxTotal"] as? Int ?? 0
        pageSizeField.stringValue = ps > 0 ? String(ps) : ""
        pageSizeField.placeholderString = "\(searchDefault) — the default (team.json search_defaults.max_results_search)"
        pageSizeField.toolTip = "maxResults of ONE search request; more pages follow until every issue is fetched"
        maxTotalField.stringValue = mt > 0 ? String(mt) : ""
        maxTotalField.toolTip = "Stop after this many issues (the newest updated first)"
        let form: [(String, NSView)] = [
            ("Name", nameField), ("Defined in", sourceRow), ("Writes", fileLabel), ("Type", typePopup), ("Every", everyBox),
            ("", enabledCheck), ("Projects", projectsPicker), ("Query", queryPopup), ("JQL", jqlField),
            ("Job args", argsField), ("Page size", pageSizeField), ("Max issues", maxTotalField),
        ]
        actionButton.title = "Force Poll"
        actionButton.toolTip = "Poll this job now (warns if a poll is already running)"
        // a control lives in one grid at a time: detach from the previous page
        for (_, v) in form { v.removeFromSuperview() }
        let g = grid(form)
        gridView = g
        for (_, v) in form where (v is NSTextField && v !== fileLabel) || v === projectsPicker {
            v.widthAnchor.constraint(greaterThanOrEqualToConstant: 380).isActive = true
        }
        colsTitle = sectionTitle("COLUMNS — this job's tab in the Jira window (and the fields it fetches)")
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

    private var selectedType: String { typePopup.titleOfSelectedItem ?? "issues" }

    private func updateFormVisibility() {
        let name = nameField.stringValue.trimmingCharacters(in: .whitespaces)
        let type = selectedType
        sourceLabel.stringValue = isNew
            ? "\(tilde(configPath)) › endpoints (added on Save)"
            : "\(tilde(configPath)) › endpoints › \"\(name)\""
        let outPath = (liveData?["path"] as? String).map(tilde)
        fileLabel.stringValue = type == "directory"
            ? "~/.cache/jira/directory.json — projects, users, statuses for the pickers (no tab)"
            : name.isEmpty ? "(set a name)" : (!isNew ? outPath : nil) ?? "\(name).json"
        let issues = type == "issues"
        let q = queryPopup.indexOfSelectedItem
        setRowHidden(queryPopup, !issues)
        setRowHidden(jqlField, !issues || q != 1)
        setRowHidden(argsField, !issues || q < 2)
        setRowHidden(pageSizeField, !issues)
        setRowHidden(maxTotalField, !issues)
        colsTitle.isHidden = type == "directory"
        colEditor.view.isHidden = type == "directory"
        if let pr = gridView?.cell(for: projectsPicker)?.row,
           let l = pr.cell(at: 0).contentView as? NSTextField {
            l.stringValue = type == "directory" ? "Users of" : "Projects"
        }
    }

    // status line + request text from the latest describe (never touches the form)
    private func updateLive() {
        switch current {
        case .job, .addJob: break
        case .liveSearch, .group: return
        case .connection: updateConnection(); return
        case .definitions: reloadDefinitions(); return
        }
        sidebar.reloadData()
        if let i = items.firstIndex(of: current) {
            sidebar.selectRowIndexes(IndexSet(integer: i), byExtendingSelection: false)
        }
        guard let d = liveData, !isNew else {
            editorStatus.stringValue = "New poll job — give it a name, a schedule and a query, then Save. "
                + "It becomes its own tab in the Jira window."
            editorStatus.textColor = .secondaryLabelColor
            requestText.string = "Save to see the exact JQL and curl requests."
            return
        }
        let name = d["name"] as? String ?? ""
        let type = d["type"] as? String ?? "issues"
        var st = d["status"] as? String ?? ""
        if JiraPoll.running.contains(name) || JiraPoll.running.contains("*") { st = "running" }
        // "● 475 items · updated 06:50 · next 07:00" — the status word only
        // when it isn't plain ok; the fetch window goes to the tooltip
        var parts: [String] = []
        let items = (d["items"] as? Int).map { type == "directory" ? "\($0) users" : "\($0) items" }
        if st == "ok", let items { parts.append("● \(items)") } else {
            parts.append("● \(st)")
            if let items { parts.append(items) }
        }
        if let lr = d["lastRun"] as? String, !lr.isEmpty { parts.append("updated \(JiraPoll.short(lr))") }
        parts.append("next \(JiraPoll.short(d["nextRun"] as? String))")
        editorStatus.stringValue = parts.joined(separator: "  ·  ")
        editorStatus.toolTip = type == "issues"
            ? "Next poll fetches issues updated since \(d["nextWindow"] as? String ?? "?") (every \(d["window"] as? String ?? "?"))"
            : "Runs every \(d["window"] as? String ?? "?")"
        editorStatus.textColor = statusColor(st) == .tertiaryLabelColor ? .secondaryLabelColor : statusColor(st)

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
            head("JQL (next run)")
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

    // "" -> 0 (= the default); anything else must be a whole number
    private func limit(_ f: NSTextField, _ what: String) -> Int? {
        let s = f.stringValue.trimmingCharacters(in: .whitespaces)
        if s.isEmpty { return 0 }
        guard let n = Int(s), n >= 0 else {
            editorMsg.textColor = .systemRed
            editorMsg.stringValue = "✗ \(what) must be a whole number (empty = default)"
            return nil
        }
        return n
    }

    private func draftJSON() -> [String: Any]? {
        window.makeFirstResponder(nil)   // commit an in-progress edit
        guard let ps = limit(pageSizeField, "Page size"), let mt = limit(maxTotalField, "Max issues") else { return nil }
        let name = nameField.stringValue.trimmingCharacters(in: .whitespaces)
        let type = selectedType
        var o: [String: Any] = ["name": name, "type": type, "maxResults": ps, "maxTotal": mt,
                                "window": everyBox.stringValue.trimmingCharacters(in: .whitespaces),
                                "enabled": enabledCheck.state == .on]
        if type != "directory" { o["columns"] = ListColumn.serialize(colEditor.cols, titles: false) }
        let picked = projectsPicker.selected
        o["projects"] = projectsPicker.isAll || picked.isEmpty ? "*" as Any : picked as Any
        let q = type == "issues" ? queryPopup.indexOfSelectedItem : 0
        o["jql"] = q == 1 ? jqlField.stringValue.trimmingCharacters(in: .whitespaces) : ""
        o["job"] = q >= 2 ? String((queryPopup.titleOfSelectedItem ?? "").dropFirst("team.json: ".count)) : ""
        o["args"] = q >= 2 ? parseArgs(argsField.stringValue) : [String: String]()
        return o
    }

    @objc private func save(_ sender: Any?) {
        guard dirty || isNew else { return }
        switch current {
        case .job, .addJob: persist(then: nil)
        case .liveSearch: persistLive()
        default: return
        }
    }

    // validate + write via jira_config.py; `then` runs after a successful save
    private func persist(then: ((String) -> Void)?) {
        guard let o = draftJSON(), let data = try? JSONSerialization.data(withJSONObject: o) else { return }
        let name = o["name"] as? String ?? ""
        editorMsg.textColor = .secondaryLabelColor
        editorMsg.stringValue = "Saving…"
        JiraPoll.run("jira_config.py", ["--upsert-endpoint"],
                     stdin: String(decoding: data, as: UTF8.self)) { [weak self] code, out, err in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            guard code == 0, r["ok"] as? Bool == true else {
                let probs = r["problems"] as? [String] ?? [JiraPoll.errorLine(err, fallback: "save failed (exit \(code))")]
                self.editorMsg.textColor = .systemRed
                self.editorMsg.stringValue = "✗ " + probs.joined(separator: "\n✗ ")
                return
            }
            self.controller?.log("jira: saved job \(name)")
            self.dirty = false
            self.isNew = false
            self.current = .job(name)
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
        a.messageText = "Delete poll job “\(name)”?"
        a.informativeText = (d["type"] as? String) == "directory"
            ? "It is removed from config.json; the pickers keep the last cached lists but they stop refreshing."
            : "It is removed from config.json and its tab (\(d["file"] as? String ?? "")) disappears from the Jira window."
        a.addButton(withTitle: "Delete")
        a.addButton(withTitle: "Cancel")
        ask(a) { [weak self] ok in if ok { self?.performDelete(name, d) } }
    }

    private func performDelete(_ name: String, _ d: [String: Any]) {
        let path = (d["type"] as? String) == "directory" ? nil : tabPath(d)
        JiraPoll.run("jira_config.py", ["--delete-endpoint", name]) { [weak self] code, out, _ in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            guard code == 0, r["ok"] as? Bool == true else {
                self.editorMsg.textColor = .systemRed
                self.editorMsg.stringValue = "✗ " + (r["problems"] as? [String] ?? ["delete failed"]).joined(separator: "; ")
                return
            }
            // the tab file goes with it (otherwise it lingers as a stale tab)
            if let path { try? FileManager.default.removeItem(atPath: path) }
            self.controller?.log("jira: deleted job \(name)")
            self.dirty = false
            self.didInitialSelect = false
            self.controller?.reloadJiraWindow()
            self.refresh()
        }
    }

    // absolute path of a job's tab file
    private func tabPath(_ d: [String: Any]) -> String? {
        if let p = d["path"] as? String, !p.isEmpty { return p }
        guard let f = d["file"] as? String, !f.isEmpty,
              let p = eps.first?["path"] as? String else { return nil }
        return ((p as NSString).deletingLastPathComponent as NSString).appendingPathComponent(f)
    }

    @objc private func primaryAction(_ sender: Any?) {
        if dirty {
            // Force Poll acts on the SAVED definition: save first
            persist { [weak self] _ in self?.primaryAction(nil) }
            return
        }
        guard let name = editingName else { return }
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
            self?.dir = JiraDirectory.load()
            self?.refresh()
        }
        updateButtons()
        updateLive()
        refresh()
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

    // MARK: live search settings

    private func showLiveSearch() {
        isNew = false
        let ls = info["liveSearch"] as? [String: Any] ?? [:]
        loadColumns(ls["columnsSpec"] as? String, me: "\u{0}live")
        liveMaxField.stringValue = (ls["maxResults"] as? Int).map(String.init) ?? ""
        liveMaxField.removeFromSuperview()
        liveMaxField.widthAnchor.constraint(equalToConstant: 90).isActive = true
        let g = grid([("Max results", liveMaxField)])
        gridView = g
        let about = hint("⌘F in the Jira window opens the search bar: free text, projects, and "
            + "“+ Filter” rows (assignee, reporter, status, type, priority, dates, labels, any column) "
            + "picked from the cached directory. Results land in the Jira window's “search.json” tab; "
            + "these are that tab's columns. Max results = issues per search (the bar can override it).")
        for b in [saveButton, revertButton] { b.removeFromSuperview() }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow - 30, for: .horizontal)
        let buttons = row([saveButton, revertButton, spacer])
        colEditor.view.removeFromSuperview()
        editorMsg.removeFromSuperview()
        editorMsg.stringValue = ""
        colEditor.view.isHidden = false
        let page = vstack([sectionTitle("LIVE SEARCH"), about, g,
                           sectionTitle("COLUMNS — the search.json tab (and the fields a search fetches)"),
                           colEditor.view, editorMsg, buttons], spacing: 8)
        for v in [about, colEditor.view, editorMsg, buttons] as [NSView] {
            v.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        }
        colEditor.view.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        showPage(page)
        updateButtons()
    }

    private func persistLive() {
        window.makeFirstResponder(nil)
        guard let m = limit(liveMaxField, "Max results") else { return }
        var o: [String: Any] = ["columns": ListColumn.serialize(colEditor.cols, titles: false)]
        o["maxResults"] = m
        guard let data = try? JSONSerialization.data(withJSONObject: o) else { return }
        JiraPoll.run("jira_config.py", ["--set-live-search"], stdin: String(decoding: data, as: UTF8.self)) { [weak self] code, out, err in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            guard code == 0, r["ok"] as? Bool == true else {
                let probs = r["problems"] as? [String] ?? [JiraPoll.errorLine(err, fallback: "save failed (exit \(code))")]
                self.editorMsg.textColor = .systemRed
                self.editorMsg.stringValue = "✗ " + probs.joined(separator: "\n✗ ")
                return
            }
            self.dirty = false
            self.updateButtons()
            self.controller?.reloadJiraWindow()
            self.refresh {
                self.editorMsg.textColor = .systemGreen
                self.editorMsg.stringValue = "✓ Saved to config.json"
            }
        }
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
        team.json   \(s("teamPath"))\((info["teamExists"] as? Bool ?? false) ? "" : "  (not created — Definitions creates it on the first save)")
        status      \(s("statusPath"))
        directory   \(JiraPoll.directoryPath)
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

    // MARK: definitions (team.json + the directory cache)

    private func setupDefinitions() {
        defSeg.segmentCount = DefTab.allCases.count
        for (i, t) in DefTab.allCases.enumerated() {
            defSeg.setLabel(t.rawValue, forSegment: i)
            defSeg.setWidth(0, forSegment: i)
        }
        defSeg.trackingMode = .selectOne
        defSeg.selectedSegment = 0
        defSeg.segmentStyle = .automatic
        defSeg.controlSize = .small
        defSeg.target = self
        defSeg.action = #selector(defTabChanged(_:))
        defTable.dataSource = self
        defTable.delegate = self
        defTable.usesAlternatingRowBackgroundColors = true
        defTable.rowHeight = 22
        defTable.allowsMultipleSelection = true
        defTable.target = self
        defTable.doubleAction = #selector(defEditClicked(_:))
        defTable.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        button(defAdd, #selector(defAddClicked(_:)))
        button(defEdit, #selector(defEditClicked(_:)))
        button(defRemove, #selector(defRemoveClicked(_:)))
        button(defFetch, #selector(defFetchClicked(_:)), tip: "Run the directory job now (projects, users, statuses, fields)")
        defMsg.font = .systemFont(ofSize: 12)
        defHint.font = .systemFont(ofSize: 11)
        defHint.textColor = .secondaryLabelColor
    }

    private func showDefinitions() {
        let sv = NSScrollView()
        sv.documentView = defTable
        sv.hasVerticalScroller = true
        sv.hasHorizontalScroller = true
        sv.borderType = .bezelBorder
        sv.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        // no intrinsic height: without a floor the stack squeezed the table
        // down to its header row
        sv.heightAnchor.constraint(greaterThanOrEqualToConstant: 260).isActive = true
        for v in [defSeg, defHint, defMsg, defAdd, defEdit, defRemove, defFetch] as [NSView] { v.removeFromSuperview() }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow - 30, for: .horizontal)
        let buttons = row([defAdd, defEdit, defRemove, spacer, defFetch])
        let page = vstack([sectionTitle("DEFINITIONS — team.json + what the directory job cached"), defSeg,
                           defHint, sv, defMsg, buttons], spacing: 8)
        for v in [defHint, sv, defMsg, buttons] as [NSView] {
            v.widthAnchor.constraint(equalTo: page.widthAnchor).isActive = true
        }
        showPage(page)
        defMsg.stringValue = ""
        reloadDefinitions(rebuildColumns: true)
    }

    @objc private func defTabChanged(_ sender: Any?) {
        defTab = DefTab.allCases[max(0, defSeg.selectedSegment)]
        defMsg.stringValue = ""
        reloadDefinitions(rebuildColumns: true)
    }

    // merged with the defaults (display) / team.json's own values (edits)
    private var team: [String: Any] { info["team"] as? [String: Any] ?? [:] }
    private var teamOwn: [String: Any] { info["teamOwn"] as? [String: Any] ?? [:] }
    private func own(_ key: String) -> [String: Any] { teamOwn[key] as? [String: Any] ?? [:] }

    private func defColumns() -> [(String, String, CGFloat)] {
        switch defTab {
        case .projects: return [("a", "Project key", 120), ("b", "Name", 260), ("c", "Users cached", 120)]
        case .fields: return [("a", "Field", 140), ("b", "Label", 170), ("c", "Jira field", 170),
                              ("d", "Kind", 80), ("e", "Used by", 170), ("f", "Has data in", 160)]
        case .users: return [("a", "Name", 200), ("b", "ID (used in JQL)", 170), ("c", "Email", 200), ("d", "Projects", 200)]
        case .lists: return [("a", "Kind", 120), ("b", "Value", 300)]
        case .api: return [("a", "Name", 170), ("b", "Path", 420)]
        case .jql: return [("a", "Name", 190), ("b", "JQL", 520)]
        case .defaults: return [("a", "Setting", 190), ("b", "Value", 90), ("c", "Meaning", 420)]
        }
    }

    private static let defaultMeaning: [String: String] = [
        "max_results_search": "page size of one issue search request (jobs without their own Page size)",
        "max_results_users": "page size of one user-directory request (directory job)",
        "page_size": "page size of one agile board request",
        "timeout_seconds": "curl -m: seconds before a request gives up",
        "cache_timeout_seconds": "re-use versions.json this long (0 = always refetch)",
        "versions_lookback_days": "drop releases dated older than this (0 = keep all)",
        "labels_max_issues": "directory job: newest labelled issues scanned per project for the ⌘F Labels picker (0 = skip)",
    ]

    private func str(_ v: Any?) -> String {
        switch v {
        case let s as String: return s
        case let n as NSNumber: return n.stringValue
        case nil: return ""
        default:
            let d = (try? JSONSerialization.data(withJSONObject: v!, options: [.fragmentsAllowed])) ?? Data()
            return String(decoding: d, as: UTF8.self)
        }
    }

    private func reloadDefinitions(rebuildColumns: Bool = false) {
        guard current == .definitions else { return }
        if rebuildColumns {
            while let c = defTable.tableColumns.first { defTable.removeTableColumn(c) }
            for (id, title, w) in defColumns() {
                let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
                col.title = title
                col.width = w
                col.minWidth = 40
                defTable.addTableColumn(col)
            }
        }
        var rows: [[String]] = [], keys: [String] = []
        switch defTab {
        case .projects:
            let names = Dictionary(dir.projects.map { ($0.key, $0.name) }, uniquingKeysWith: { a, _ in a })
            for k in team["project_keys"] as? [String] ?? [] {
                let n = dir.users.filter { $0.projects.contains(k) }.count
                rows.append([k, names[k] ?? (dir.isEmpty ? "" : "(not visible to the token)"), dir.isEmpty ? "" : "\(n)"])
                keys.append(k)
            }
            defHint.stringValue = "The team's projects (team.json project_keys): the default scope of every "
                + "job and search, and whose users the directory job caches. Add picks from the projects "
                + "your token can see."
        case .fields:
            for c in catalog {
                let f = str(c["field"])
                let label = str(c["label"]).isEmpty ? (JiraPoll.baseFieldLabels[f] ?? f) : str(c["label"])
                let kind = c["custom"] as? Bool == true ? "custom" : c["base"] as? Bool == true ? "built-in" : "Jira field"
                rows.append([f, label + (c["renamed"] as? Bool == true ? "  ✎" : ""),
                             (c["apiFields"] as? [String] ?? []).joined(separator: ", "), kind,
                             (c["usedBy"] as? [String] ?? []).joined(separator: ", "),
                             (c["seenIn"] as? [String] ?? []).joined(separator: ", ")])
                keys.append(f)
            }
            defHint.stringValue = "Every field a column can show, each with ONE label — the header in every "
                + "job, the search tab and the ⌘F filters. Double-click to rename (✎ = renamed). Add Custom "
                + "Field maps a Jira custom field. Pick which columns a tab shows in its job (Poll Jobs / Live Search)."
        case .users:
            for u in dir.users {
                rows.append([u.name, u.id, u.email, u.projects.joined(separator: ", ")])
                keys.append(u.id)
            }
            defHint.stringValue = dir.isEmpty
                ? "Nothing cached yet — the weekly directory job fills this (Fetch from Jira now)."
                : "\(dir.users.count) assignable users of the team's projects, cached \(JiraPoll.short(dir.fetchedAt)) "
                    + "by the directory job — the assignee / reporter pickers."
        case .lists:
            for (kind, vals) in [("Status", dir.statuses), ("Issue type", dir.issueTypes), ("Priority", dir.priorities)] {
                for v in vals { rows.append([kind, v]); keys.append(v) }
            }
            defHint.stringValue = dir.isEmpty ? "Nothing cached yet — Fetch from Jira now."
                : "Cached \(JiraPoll.short(dir.fetchedAt)) by the directory job — the search pickers."
        case .api:
            let mine = own("api_endpoints")
            for (k, v) in (team["api_endpoints"] as? [String: Any] ?? [:]).sorted(by: { $0.key < $1.key }) {
                rows.append([k, str(v) + (mine[k] == nil ? "   (built-in default)" : "")])
                keys.append(k)
            }
            defHint.stringValue = "Every Jira REST path the tools call (relative to /rest/api/2; board* to "
                + "/rest/agile/1.0; a /rest/… path is used as-is). {name} placeholders are filled per request."
        case .jql:
            let mine = own("jql_templates")
            for (k, v) in (team["jql_templates"] as? [String: Any] ?? [:]).sorted(by: { $0.key < $1.key }) {
                rows.append([k, str(v) + (mine[k] == nil ? "   (built-in default)" : "")])
                keys.append(k)
            }
            defHint.stringValue = "JQL templates a poll job can run (Query ▸ team.json: NAME). {projects} = "
                + "the job's projects; every other {name} comes from the job's args."
        case .defaults:
            let eff = info["searchDefaults"] as? [String: Any] ?? [:]
            let mine = own("search_defaults")
            for k in eff.keys.sorted() {
                rows.append([k, str(eff[k]), (Self.defaultMeaning[k] ?? "") + (mine[k] == nil ? "" : "  (set in team.json)")])
                keys.append(k)
            }
            defHint.stringValue = "Request limits and timeouts. max_results_search is the “maxResults=50” in "
                + "every search curl — a poll job's own Page size overrides it. Reset = back to the default."
        }
        defRows = rows
        defKeys = keys
        defTable.reloadData()
        let editable = defTab.editable
        defAdd.isHidden = !editable || defTab == .defaults
        defAdd.title = defTab == .fields ? "Add Custom Field…" : "Add…"
        defEdit.title = defTab == .fields ? "Rename…" : "Edit…"
        defEdit.isHidden = !editable || defTab == .projects
        defRemove.isHidden = !editable
        defRemove.title = defTab == .defaults ? "Reset to Default" : defTab == .fields ? "Remove / Reset" : "Remove"
        defFetch.isHidden = ![.projects, .users, .lists, .fields].contains(defTab)
    }

    private func defCell(_ id: String, _ row: Int) -> NSView? {
        guard row < defRows.count else { return nil }
        let i = Int((id.unicodeScalars.first?.value ?? 97) - 97)
        let s = i < defRows[row].count ? defRows[row][i] : ""
        let l = NSTextField(labelWithString: s)
        l.lineBreakMode = .byTruncatingTail
        l.toolTip = s
        if i == 0 || [.api, .jql].contains(defTab) || (defTab == .fields && i == 2) {
            l.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        }
        if i > 0 && !(defTab == .jql || defTab == .api) { l.textColor = .secondaryLabelColor }
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

    // write one team.json key (validated by jira_config.py --team-set)
    private func teamSet(_ key: String, _ value: Any, done: String, then: (() -> Void)? = nil) {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed]) else { return }
        defMsg.textColor = .secondaryLabelColor
        defMsg.stringValue = "Saving…"
        JiraPoll.run("jira_config.py", ["--team-set", key], stdin: String(decoding: data, as: UTF8.self)) { [weak self] code, out, err in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            guard code == 0, r["ok"] as? Bool == true else {
                let probs = r["problems"] as? [String] ?? [JiraPoll.errorLine(err, fallback: "save failed (exit \(code))")]
                self.defMsg.textColor = .systemRed
                self.defMsg.stringValue = "✗ " + probs.joined(separator: "\n✗ ")
                return
            }
            self.controller?.log("jira: team.json \(key) — \(done)")
            // labels are the Jira window's column headers
            if ["field_labels", "custom_fields"].contains(key) { self.controller?.reloadJiraWindow() }
            if let then { then(); return }
            self.refresh {
                self.defMsg.textColor = .systemGreen
                self.defMsg.stringValue = "✓ \(done) — saved to team.json"
            }
        }
    }

    private var defSelection: [Int] { defTable.selectedRowIndexes.filter { $0 < defKeys.count } }

    @objc private func defAddClicked(_ sender: Any?) {
        switch defTab {
        case .projects: addProjects()
        case .fields: editCustomField(nil)
        case .api, .jql: editKeyValue(nil)
        default: break
        }
    }

    private var customAliases: Set<String> { Set((team["custom_fields"] as? [String: Any] ?? [:]).keys) }

    @objc private func defEditClicked(_ sender: Any?) {
        guard defTab.editable, defTab != .projects else { return }
        let r = sender is NSTableView ? defTable.clickedRow : (defSelection.first ?? -1)
        guard r >= 0, r < defKeys.count else { return }
        switch defTab {
        case .fields:
            customAliases.contains(defKeys[r]) ? editCustomField(defKeys[r]) : editFieldLabel(defKeys[r])
        case .api, .jql: editKeyValue(defKeys[r])
        case .defaults: editDefault(defKeys[r])
        default: break
        }
    }

    @objc private func defRemoveClicked(_ sender: Any?) {
        let keys = defSelection.map { defKeys[$0] }
        guard !keys.isEmpty else { return }
        let what = keys.count == 1 ? "“\(keys[0])”" : "\(keys.count) entries"
        let a = NSAlert()
        a.messageText = defTab == .defaults ? "Reset \(what) to the default?" : "Remove \(what) from \(defTab.rawValue)?"
        a.informativeText = "team.json is updated right away."
        a.addButton(withTitle: defTab == .defaults ? "Reset" : "Remove")
        a.addButton(withTitle: "Cancel")
        ask(a) { [weak self] ok in
            guard ok, let self else { return }
            switch self.defTab {
            case .projects:
                self.teamSet("project_keys", (self.team["project_keys"] as? [String] ?? []).filter { !keys.contains($0) },
                             done: "removed \(what)")
            case .fields:
                // custom fields are removed; a renamed field goes back to its default label
                var cf = self.own("custom_fields"), fl = self.own("field_labels")
                let drop = keys.filter { cf[$0] != nil }, reset = keys.filter { fl[$0] != nil }
                guard !drop.isEmpty || !reset.isEmpty else {
                    self.defMsg.textColor = .secondaryLabelColor
                    self.defMsg.stringValue = "\(keys.joined(separator: ", ")): already the default label — Rename… to change it"
                    return
                }
                for k in drop { cf.removeValue(forKey: k) }
                for k in reset { fl.removeValue(forKey: k) }
                let labels = { self.teamSet("field_labels", fl, done: "reset \(reset.joined(separator: ", "))") }
                if drop.isEmpty { labels() } else {
                    self.teamSet("custom_fields", cf, done: "removed \(drop.joined(separator: ", "))",
                                 then: reset.isEmpty ? nil : labels)
                }
            default:
                guard let tk = self.teamKey else { return }
                // built-in defaults can't be removed (they come back): only team.json's own
                var d = self.own(tk)
                let builtIn = keys.filter { d[$0] == nil && tk != "custom_fields" }
                if !builtIn.isEmpty && builtIn.count == keys.count {
                    self.defMsg.textColor = .secondaryLabelColor
                    self.defMsg.stringValue = "\(builtIn.joined(separator: ", ")): built-in default — edit it to override"
                    return
                }
                for k in keys { d.removeValue(forKey: k) }
                self.teamSet(tk, d, done: self.defTab == .defaults ? "reset \(what)" : "removed \(what)")
            }
        }
    }

    private var teamKey: String? {
        switch defTab {
        case .projects: return "project_keys"
        case .api: return "api_endpoints"
        case .jql: return "jql_templates"
        case .defaults: return "search_defaults"
        default: return nil
        }
    }

    @objc private func defFetchClicked(_ sender: Any?) {
        defFetch.isEnabled = false
        defMsg.textColor = .secondaryLabelColor
        defMsg.stringValue = "Fetching projects, users, statuses, fields… (one call per project)"
        JiraPoll.run("jira_poll.py", ["--directory", "--quiet"]) { [weak self] code, _, err in
            guard let self else { return }
            self.defFetch.isEnabled = true
            if code == 0 {
                self.dir = JiraDirectory.load()
                self.reloadDefinitions()
                self.defMsg.textColor = .systemGreen
                self.defMsg.stringValue = "✓ \(self.dir.users.count) users · \(self.dir.projects.count) projects · "
                    + "\(self.dir.fields.count) fields cached"
                self.refresh()
            } else {
                self.defMsg.textColor = .systemRed
                self.defMsg.stringValue = "✗ " + JiraPoll.errorLine(err, fallback: "directory failed (exit \(code))")
            }
        }
    }

    // projects: pick from what the token can see (typed keys only when the
    // directory has never run)
    private func addProjects() {
        let have = team["project_keys"] as? [String] ?? []
        if dir.projects.isEmpty {
            let f = NSTextField()
            f.placeholderString = "KEY1, KEY2 (Fetch from Jira now to pick from a list instead)"
            jiraFormSheet(on: window, title: "Add Projects", info: "Project keys, comma separated.",
                          rows: [("Keys", f)], ok: "Add") { [weak self] ok in
                guard ok, let self else { return }
                let add = f.stringValue.uppercased().split(separator: ",")
                    .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty && !have.contains($0) }
                guard !add.isEmpty else { return }
                self.teamSet("project_keys", have + add, done: "added \(add.joined(separator: ", "))")
            }
            return
        }
        let p = JiraMultiPicker(noun: "project")
        p.options = dir.projectOptions().filter { !have.contains($0.id) }
        p.placeholder = "Choose projects…"
        jiraFormSheet(on: window, title: "Add Projects",
                      info: "Projects your token can see (directory cache). Their users are cached on the next directory run.",
                      rows: [("Projects", p)], ok: "Add", first: p) { [weak self] ok in
            guard ok, let self, !p.selected.isEmpty else { return }
            self.teamSet("project_keys", have + p.selected, done: "added \(p.selected.joined(separator: ", "))")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { p.togglePopover(nil) }
    }

    private func snakeCase(_ s: String) -> String {
        let parts = s.lowercased().split(whereSeparator: { !$0.isLetter && !$0.isNumber })
        var out = parts.joined(separator: "_")
        if let f = out.first, !f.isLetter { out = "f_" + out }
        return out
    }

    private func editCustomField(_ alias: String?) {
        let cur = alias.flatMap { own("custom_fields")[$0] as? [String: Any] ?? (team["custom_fields"] as? [String: Any])?[$0] as? [String: Any] } ?? [:]
        let customs = dir.fields.filter { $0.custom }
        let field = NSComboBox()
        field.completes = true
        field.numberOfVisibleItems = 16
        field.addItems(withObjectValues: customs.map { "\($0.name) — \($0.id)" })
        let curID = str(cur["field_id"] ?? cur["id"])
        field.stringValue = curID.isEmpty ? "" : (customs.first { $0.id == curID }.map { "\($0.name) — \($0.id)" } ?? curID)
        field.placeholderString = customs.isEmpty ? "customfield_NNNNN (Fetch from Jira now to pick by name)"
            : "type to find a Jira field by name"
        let aliasF = NSTextField(string: alias ?? "")
        aliasF.placeholderString = "column / filter name, e.g. package_info"
        aliasF.isEditable = alias == nil
        let label = NSTextField(string: str(cur["label"]))
        label.placeholderString = "header / display name"
        let desc = NSTextField(string: str(cur["description"]))
        desc.placeholderString = "optional"
        jiraFormSheet(on: window, title: alias == nil ? "Add Custom Field" : "Edit Custom Field “\(alias!)”",
                      info: "Maps a Jira custom field to a friendly alias you can use as a column and as a search filter.",
                      rows: [("Jira field", field), ("Alias", aliasF), ("Label", label), ("Description", desc)],
                      first: alias == nil ? field : label) { [weak self] ok in
            guard ok, let self else { return }
            let raw = field.stringValue
            var fid = raw.range(of: #"customfield_\d+"#, options: .regularExpression).map { String(raw[$0]) } ?? ""
            if fid.isEmpty, let hit = customs.first(where: { $0.name.caseInsensitiveCompare(raw) == .orderedSame }) {
                fid = hit.id
            }
            guard !fid.isEmpty else {
                self.defMsg.textColor = .systemRed
                self.defMsg.stringValue = "✗ pick a Jira custom field (or type its customfield_NNNNN id)"
                return
            }
            let jiraName = customs.first { $0.id == fid }?.name ?? ""
            let lbl = label.stringValue.trimmingCharacters(in: .whitespaces).isEmpty ? jiraName
                : label.stringValue.trimmingCharacters(in: .whitespaces)
            var a = aliasF.stringValue.trimmingCharacters(in: .whitespaces)
            if a.isEmpty { a = self.snakeCase(lbl.isEmpty ? fid : lbl) }
            var d = self.own("custom_fields")
            var entry: [String: Any] = ["field_id": fid, "label": lbl.isEmpty ? a : lbl]
            let ds = desc.stringValue.trimmingCharacters(in: .whitespaces)
            if !ds.isEmpty { entry["description"] = ds }
            d[a] = entry
            // the custom field's own label IS its one label: drop an old rename
            var fl = self.own("field_labels")
            let clear = fl.removeValue(forKey: a) != nil
            self.teamSet("custom_fields", d, done: alias == nil ? "added \(a)" : "updated \(a)",
                         then: clear ? { self.teamSet("field_labels", fl, done: "label of \(a)") } : nil)
        }
    }

    // a built-in / Jira field's one label (team.json field_labels)
    private func editFieldLabel(_ f: String) {
        let c = catalog.first { str($0["field"]) == f } ?? [:]
        let def = str(c["defaultLabel"]).isEmpty ? (JiraPoll.baseFieldLabels[f] ?? f) : str(c["defaultLabel"])
        let cur = own("field_labels")[f] as? String ?? ""
        let l = NSTextField(string: cur)
        l.placeholderString = def
        jiraFormSheet(on: window, title: "Rename “\(f)”",
                      info: "One label for this field: the column header in every job, the search tab and the ⌘F "
                          + "filter. Empty = the default (\(def)).",
                      rows: [("Label", l)], first: l) { [weak self] ok in
            guard ok, let self else { return }
            var d = self.own("field_labels")
            let v = l.stringValue.trimmingCharacters(in: .whitespaces)
            if v.isEmpty || v == def { d.removeValue(forKey: f) } else { d[f] = v }
            self.teamSet("field_labels", d, done: v.isEmpty || v == def ? "\(f) back to “\(def)”" : "\(f) → “\(v)”")
        }
    }

    private func editKeyValue(_ key: String?) {
        guard let tk = teamKey else { return }
        let d = team[tk] as? [String: Any] ?? [:]      // shows built-in values too
        let k = NSTextField(string: key ?? "")
        k.placeholderString = tk == "api_endpoints" ? "e.g. components" : "e.g. my_bugs"
        k.isEditable = key == nil
        let v = NSTextField(string: key.map { str(d[$0]) } ?? "")
        v.placeholderString = tk == "api_endpoints" ? "/project/{project}/components"
            : "project in ({projects}) AND assignee = currentUser()"
        v.widthAnchor.constraint(greaterThanOrEqualToConstant: 520).isActive = true
        let single = tk == "api_endpoints" ? "API endpoint" : "JQL template"
        jiraFormSheet(on: window, title: key == nil ? "Add \(single)" : "Edit \(single) “\(key!)”",
                      info: tk == "api_endpoints" ? "A REST path relative to /rest/api/2 ({name} = filled per request)."
                          : "{projects} = the job's projects; other {name}s come from the job's args.",
                      rows: [("Name", k), (tk == "api_endpoints" ? "Path" : "JQL", v)],
                      first: key == nil ? k : v) { [weak self] ok in
            guard ok, let self else { return }
            let name = k.stringValue.trimmingCharacters(in: .whitespaces)
            let val = v.stringValue.trimmingCharacters(in: .whitespaces)
            guard !name.isEmpty, !val.isEmpty else { NSSound.beep(); return }
            var nd = self.own(tk)
            nd[name] = val
            self.teamSet(tk, nd, done: key == nil ? "added \(name)" : "updated \(name)")
        }
    }

    private func editDefault(_ key: String) {
        let eff = info["searchDefaults"] as? [String: Any] ?? [:]
        let v = NSTextField(string: str(eff[key]))
        jiraFormSheet(on: window, title: "Edit \(key)", info: Self.defaultMeaning[key] ?? "",
                      rows: [("Value", v)], first: v) { [weak self] ok in
            guard ok, let self else { return }
            guard let n = Int(v.stringValue.trimmingCharacters(in: .whitespaces)), n >= 0 else {
                self.defMsg.textColor = .systemRed
                self.defMsg.stringValue = "✗ \(key) must be a whole number"
                return
            }
            var d = self.own("search_defaults")
            d[key] = n
            self.teamSet("search_defaults", d, done: "\(key) = \(n)")
        }
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
