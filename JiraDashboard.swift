import AppKit

// MARK: - Jira Config window (the one place for everything jira-poll)
//
// Menu bar "Open Jira Config Window" / `workspace-switcher jira-poll dashboard`.
// Everything shown comes from ONE python call — `jira_poll.py --describe` —
// so the window, jira-doctor and the poller always agree:
//   Poll Jobs   every endpoint in ~/.config/jira/config.json: on/off, its tab
//               (json file), interval, status, last/next run, the NEXT query
//               window, and (detail pane) the full JQL + the FULL curl of
//               every request it makes. Poll now / full resync / cancel.
//   Columns     the [jira] `columns` (the table in the jira window AND the
//               API fields= list): edit title/width/align/sort/filter, add,
//               remove, reorder, save back to commands.conf.
//   Connection  site, auth mode, paths, the login-test curl.
// Edits go through the same writers as before (jira_config.py --set-window /
// --set-enabled, saveConfigValue for columns) — config stays the source of
// truth; the window is a view + editor over it.
// Sizes / refresh: [jira] dashboard-width, dashboard-height, dashboard-refresh.

final class JiraDashboardWindow: NSObject, NSWindowDelegate, NSTableViewDataSource,
                                 NSTableViewDelegate, NSTextFieldDelegate {
    private static var live: JiraDashboardWindow?

    private weak var controller: SwitcherController?
    private let window: NSWindow
    private var monitor: Any?
    private var timer: Timer?
    private var describing = false
    private var pages: [NSView] = []

    // data (from jira_poll.py --describe)
    private var info: [String: Any] = [:]
    private var eps: [[String: Any]] = []
    private var cols: [ListColumn] = []         // editable copy of [jira] columns
    private var colsDirty = false
    private var colMeta: [String: [String: Any]] = [:]   // field -> apiFields/label

    // header
    private let statusLine = NSTextField(labelWithString: "Loading…")
    private let problemsLine = NSTextField(wrappingLabelWithString: "")
    private let enableButton = NSButton(title: "Enable Jira", target: nil, action: nil)
    private let pollAllButton = NSButton(title: "Poll All Now", target: nil, action: nil)
    private let cancelButton = NSButton(title: "Cancel Poll", target: nil, action: nil)
    private let openMenu = NSPopUpButton(frame: .zero, pullsDown: true)

    // poll jobs tab
    private let jobs = NSTableView()
    private let detail = NSTextView()
    private let pollOneButton = NSButton(title: "Poll Now", target: nil, action: nil)
    private let resyncButton = NSButton(title: "Full Resync", target: nil, action: nil)
    private let copyCurlButton = NSButton(title: "Copy curl", target: nil, action: nil)
    private let copyJQLButton = NSButton(title: "Copy JQL", target: nil, action: nil)

    // columns tab
    private let colTable = NSTableView()
    private let addField = NSComboBox()
    private let colsNote = NSTextField(wrappingLabelWithString: "")
    private let saveColsButton = NSButton(title: "Save Columns", target: nil, action: nil)
    private let revertColsButton = NSButton(title: "Revert", target: nil, action: nil)

    // connection tab
    private let connText = NSTextView()
    private let connResult = NSTextField(wrappingLabelWithString: "")

    private static let jobCols: [(id: String, title: String, width: CGFloat)] = [
        ("on", "On", 34), ("name", "Job", 90), ("file", "Tab (file)", 110), ("type", "Type", 62),
        ("window", "Every", 78), ("status", "Status", 80), ("lastRun", "Last run", 110),
        ("nextRun", "Next run", 110), ("nextWindow", "Next window (updated ≥)", 150),
        ("items", "Items", 50),
    ]
    private static let colCols: [(id: String, title: String, width: CGFloat)] = [
        ("field", "Field", 120), ("title", "Title", 120), ("width", "Width %", 60),
        ("align", "Align", 80), ("sort", "Sort", 40), ("filter", "Filter", 44),
        ("api", "API field(s) fetched", 170), ("label", "Custom field label", 160),
    ]

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
        window.minSize = NSSize(width: 760, height: 480)
        // same level as the setup window: above the popup windows, which
        // float at .popUpMenu
        window.level = NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 1)
        window.delegate = self
        window.contentView = buildContent()
        installKeys()
    }

    // MARK: layout

    private func button(_ b: NSButton, _ action: Selector, tip: String? = nil) -> NSButton {
        b.bezelStyle = .rounded
        b.target = self
        b.action = action
        b.toolTip = tip
        return b
    }

    private func row(_ views: [NSView]) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .horizontal
        s.spacing = 8
        s.alignment = .centerY
        s.setHuggingPriority(.required, for: .vertical)   // never stretch a button row
        return s
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

    private func tableScroll(_ t: NSTableView,
                             _ spec: [(id: String, title: String, width: CGFloat)]) -> NSScrollView {
        for c in spec {
            let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(c.id))
            col.title = c.title
            col.width = c.width
            col.minWidth = 30
            t.addTableColumn(col)
        }
        t.dataSource = self
        t.delegate = self
        t.usesAlternatingRowBackgroundColors = true
        t.rowHeight = 24
        t.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        let sv = NSScrollView()
        sv.documentView = t
        sv.hasVerticalScroller = true
        sv.hasHorizontalScroller = true
        sv.borderType = .bezelBorder
        return sv
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

    private func vstack(_ views: [NSView], spacing: CGFloat = 8) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .vertical
        s.alignment = .leading
        s.distribution = .fill     // the lowest-hugging child takes the slack
        s.spacing = spacing
        for v in views { v.translatesAutoresizingMaskIntoConstraints = false }
        return s
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

        let header = row([
            button(enableButton, #selector(toggleEnabled(_:)), tip: "[jira] enabled — the window + the launchd poll agent"),
            button(pollAllButton, #selector(pollAll(_:)), tip: "Run every enabled job now (jira_poll.py --force)"),
            button(cancelButton, #selector(cancelPoll(_:)), tip: "Stop the running poll (jira_poll.py --cancel)"),
            button(NSButton(title: "Setup…", target: nil, action: nil), #selector(setup(_:)),
                   tip: "Site, token, auth"),
            openMenu,
            button(NSButton(title: "Refresh", target: nil, action: nil), #selector(refreshClicked(_:)),
                   tip: "Re-read config + status (⌘R)"),
        ])

        // pages: a segmented switch over a plain container (NSTabView sizes
        // itself from its content's fitting size and fights the layout)
        pages = [buildJobsTab(), buildColumnsTab(), buildConnectionTab()]
        let tabs = NSView()
        for (i, pg) in pages.enumerated() {
            pinned(pg, in: tabs)
            pg.isHidden = i != 0
        }
        let seg = NSSegmentedControl(labels: ["Poll Jobs", "Columns", "Connection"],
                                     trackingMode: .selectOne, target: self,
                                     action: #selector(pageChanged(_:)))
        seg.selectedSegment = 0

        let top = vstack([statusLine, problemsLine, header, seg], spacing: 8)
        top.translatesAutoresizingMaskIntoConstraints = false
        // header = natural height; the page container takes the rest
        top.setHuggingPriority(.required, for: .vertical)
        for v in [statusLine, problemsLine, seg] as [NSView] {
            v.setContentHuggingPriority(.required, for: .vertical)
        }
        tabs.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(top)
        content.addSubview(tabs)
        NSLayoutConstraint.activate([
            top.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            top.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            top.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            statusLine.widthAnchor.constraint(equalTo: top.widthAnchor),
            problemsLine.widthAnchor.constraint(equalTo: top.widthAnchor),
            tabs.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10),
            tabs.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10),
            tabs.topAnchor.constraint(equalTo: top.bottomAnchor, constant: 8),
            tabs.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
        ])
        return content
    }

    private func buildJobsTab() -> NSView {
        let v = NSView()
        let hint = NSTextField(wrappingLabelWithString:
            "Each job is one entry in \"endpoints\" of the jira config.json and writes one tab (json file) of the "
            + "Jira window. launchd ticks every 60s; a job runs when its interval has passed. Select a job to see "
            + "its full JQL and the exact curl of every request.")
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = .secondaryLabelColor
        let tableSV = tableScroll(jobs, Self.jobCols)
        jobs.target = self
        jobs.doubleAction = #selector(pollSelected(_:))
        let buttons = row([
            button(pollOneButton, #selector(pollSelected(_:)), tip: "Run this job now (double-click a row too)"),
            button(resyncButton, #selector(resyncSelected(_:)), tip: "Full sync for this job (jira_poll.py --init)"),
            button(copyJQLButton, #selector(copyJQL(_:))),
            button(copyCurlButton, #selector(copyCurl(_:)), tip: "Copy every request of this job as curl (real token)"),
        ])
        let detailSV = monoTextView(detail)

        let split = NSSplitView()
        split.isVertical = false
        split.dividerStyle = .thin
        let bottom = NSView()
        let bstack = vstack([buttons, detailSV], spacing: 6)
        pinned(bstack, in: bottom)
        detailSV.widthAnchor.constraint(equalTo: bstack.widthAnchor).isActive = true
        split.addArrangedSubview(tableSV)
        split.addArrangedSubview(bottom)
        split.setHoldingPriority(.defaultLow + 1, forSubviewAt: 0)
        split.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        detailSV.heightAnchor.constraint(greaterThanOrEqualToConstant: 160).isActive = true

        let stack = vstack([hint, split], spacing: 6)
        pinned(stack, in: v, inset: 8)
        hint.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        split.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        tableSV.heightAnchor.constraint(greaterThanOrEqualToConstant: 120).isActive = true
        // table: header + ~5 rows; the detail pane gets the rest
        DispatchQueue.main.async { split.setPosition(24 * 6 + 12, ofDividerAt: 0) }
        return v
    }

    private func buildColumnsTab() -> NSView {
        let v = NSView()
        let hint = NSTextField(wrappingLabelWithString:
            "The [jira] columns in commands.conf: the table drawn in EVERY tab of the Jira window, and the "
            + "fields the poller asks Jira for (API field(s) column). Width is % of the row (0 = share the rest). "
            + "Custom fields from team.json custom_fields can be added by their alias.")
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = .secondaryLabelColor
        let sv = tableScroll(colTable, Self.colCols)
        addField.placeholderString = "field (e.g. created, duedate, customfield_10010)"
        addField.completes = true
        addField.target = self
        addField.action = #selector(addColumn(_:))
        colsNote.font = .systemFont(ofSize: 11)
        colsNote.textColor = .secondaryLabelColor
        let buttons = row([
            addField,
            button(NSButton(title: "Add", target: nil, action: nil), #selector(addColumn(_:))),
            button(NSButton(title: "Remove", target: nil, action: nil), #selector(removeColumn(_:))),
            button(NSButton(title: "▲", target: nil, action: nil), #selector(moveUp(_:)), tip: "Move left"),
            button(NSButton(title: "▼", target: nil, action: nil), #selector(moveDown(_:)), tip: "Move right"),
            button(revertColsButton, #selector(revertColumns(_:))),
            button(saveColsButton, #selector(saveColumns(_:)), tip: "Write columns back to commands.conf"),
        ])
        addField.widthAnchor.constraint(equalToConstant: 300).isActive = true
        sv.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        let stack = vstack([hint, sv, buttons, colsNote], spacing: 6)
        pinned(stack, in: v, inset: 8)
        for x in [hint, sv, colsNote] { x.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true }
        return v
    }

    private func buildConnectionTab() -> NSView {
        let v = NSView()
        let sv = monoTextView(connText)
        connResult.font = .systemFont(ofSize: 12)
        let buttons = row([
            button(NSButton(title: "Test Connection", target: nil, action: nil), #selector(testConnection(_:)),
                   tip: "GET /rest/api/2/myself with the saved config"),
            button(NSButton(title: "Copy Login curl", target: nil, action: nil), #selector(copyLoginCurl(_:))),
            button(NSButton(title: "Setup…", target: nil, action: nil), #selector(setup(_:))),
        ])
        sv.setContentHuggingPriority(.defaultLow - 20, for: .vertical)
        let stack = vstack([buttons, connResult, sv], spacing: 6)
        pinned(stack, in: v, inset: 8)
        for x in [connResult, sv] { x.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true }
        return v
    }

    // MARK: keys (rule.md #1: edit shortcuts in every field)

    private func installKeys() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.window.isKeyWindow else { return e }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command), ctrl = mods.contains(.control)
            let editing = self.window.firstResponder is NSText
                && (self.window.firstResponder as? NSTextView)?.isEditable == true
            if e.keyCode == 53 {                       // Esc: end an edit, else close
                if editing { self.window.makeFirstResponder(nil); return nil }
                self.close()
                return nil
            }
            if cmd && e.keyCode == 13 { self.close(); return nil }           // Cmd+W
            if cmd && e.keyCode == 15 { self.refresh(); return nil }         // Cmd+R
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

    @objc private func refreshClicked(_ sender: Any?) { refresh() }

    @objc private func pageChanged(_ sender: NSSegmentedControl) {
        for (i, pg) in pages.enumerated() { pg.isHidden = i != sender.selectedSegment }
    }

    func refresh() {
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
        }
    }

    private func apply(_ d: [String: Any]) {
        info = d
        let sel = jobs.selectedRow
        eps = d["endpoints"] as? [[String: Any]] ?? []
        colMeta = [:]
        for c in d["columns"] as? [[String: Any]] ?? [] {
            if let f = c["field"] as? String { colMeta[f] = c }
        }
        if !colsDirty {
            cols = ListColumn.parse(jiraConfigValue("columns"))
            colTable.reloadData()
        }
        addField.removeAllItems()
        addField.addItems(withObjectValues: (d["availableFields"] as? [String] ?? [])
            .filter { f in !cols.contains { $0.field == f } })
        updateHeader()
        jobs.reloadData()
        if sel >= 0 && sel < eps.count {
            jobs.selectRowIndexes(IndexSet(integer: sel), byExtendingSelection: false)
        } else if !eps.isEmpty && sel < 0 {
            jobs.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        }
        updateDetail()
        updateColsNote()
        updateConnection()
    }

    private func updateHeader() {
        let enabled = info["enabled"] as? Bool ?? jiraEnabledInConfig()
        let bg = info["backgroundPoll"] as? Bool ?? false
        let lock = info["lock"] as? [String: Any] ?? [:]
        let held = lock["held"] as? Bool ?? false
        var parts = [enabled ? "● Polling ON" : bg ? "◐ Jira disabled — background polling" : "○ Polling OFF"]
        parts.append("\(eps.count) job\(eps.count == 1 ? "" : "s")")
        parts.append("launchd tick \(info["tick"] as? String ?? "60s")")
        if let lr = info["lastRun"] as? String, !lr.isEmpty {
            parts.append("last run \(JiraPoll.short(lr)) \(info["status"] as? String ?? "")")
        }
        if held { parts.append("⟳ poll running (pid \(lock["pid"] ?? "?"), since \(JiraPoll.short(lock["since"] as? String)))") }
        if !JiraPoll.running.isEmpty { parts.append("started here: \(JiraPoll.running.sorted().joined(separator: ", "))") }
        statusLine.stringValue = parts.joined(separator: "  ·  ")
        statusLine.textColor = enabled ? .labelColor : .secondaryLabelColor
        var probs = info["problems"] as? [String] ?? []
        if let e = info["lastError"] as? String, !e.isEmpty { probs.append("last error: \(e)") }
        if let e = JiraPoll.lastEnableError { probs.append("enable failed: \(e)") }
        problemsLine.stringValue = probs.map { "⚠ " + $0 }.joined(separator: "\n")
        problemsLine.isHidden = probs.isEmpty
        enableButton.title = enabled ? "Disable Jira" : "Enable Jira"
        cancelButton.isEnabled = held || !JiraPoll.running.isEmpty
        pollAllButton.isEnabled = !JiraPoll.running.contains("all")

        // Open… pulldown (first item is the title)
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

    private var selected: [String: Any]? {
        let r = jobs.selectedRow
        return r >= 0 && r < eps.count ? eps[r] : nil
    }

    private func updateDetail() {
        let has = selected != nil
        [pollOneButton, resyncButton, copyCurlButton, copyJQLButton].forEach { $0.isEnabled = has }
        guard let e = selected else {
            detail.textStorage?.setAttributedString(NSAttributedString(string: "Select a job."))
            return
        }
        let name = e["name"] as? String ?? "?"
        pollOneButton.isEnabled = !JiraPoll.running.contains(name)
        let out = NSMutableAttributedString()
        let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
        let bold = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .bold)
        func head(_ s: String) {
            out.append(NSAttributedString(string: s + "\n", attributes: [
                .font: bold, .foregroundColor: NSColor.secondaryLabelColor]))
        }
        func line(_ s: String, _ color: NSColor = .labelColor) {
            out.append(NSAttributedString(string: s + "\n", attributes: [.font: mono, .foregroundColor: color]))
        }
        let projects: String = {
            if let p = e["projects"] as? [String] { return p.joined(separator: ", ") }
            let pk = info["projectKeys"] as? [String] ?? []
            return pk.isEmpty ? "* (every project the token can see)" : "* → team.json project_keys: \(pk.joined(separator: ", "))"
        }()
        head("JOB")
        line("\(name)  →  tab \(e["file"] as? String ?? "")   (\(e["path"] as? String ?? ""))")
        line("type \(e["type"] as? String ?? "") · every \(e["window"] as? String ?? "") · "
             + ((e["enabled"] as? Bool ?? true) ? "enabled" : "DISABLED") + " · projects \(projects)")
        if let j = e["job"] as? String, !j.isEmpty {
            line("from team.json job/template '\(j)' args \(e["args"] ?? [:])")
        }
        if let x = e["extraJql"] as? String, !x.isEmpty { line("extra jql (config): \(x)") }
        head("\nSTATUS")
        let st = e["status"] as? String ?? ""
        line("\(st) · last run \(e["lastRun"] as? String ?? "never") · last success "
             + "\(e["lastSuccess"] as? String ?? "never") · next run \(e["nextRun"] as? String ?? "?")"
             + ((e["items"] as? Int).map { " · \($0) items" } ?? ""),
             st == "error" ? .systemRed : .labelColor)
        if (e["type"] as? String) == "issues" {
            line("next query window: updated ≥ \(e["nextWindow"] as? String ?? "?")  "
                 + "(last success − pollMarginMinutes \(info["pollMarginMinutes"] ?? 5); no cache → full)")
        }
        if let jql = e["jql"] as? String, !jql.isEmpty {
            head("\nJQL (next run)")
            line(jql)
        }
        head("\nREQUESTS — full curl, copy/paste to run (includes the token)")
        for r in e["requests"] as? [[String: Any]] ?? [] {
            line("# \(r["purpose"] as? String ?? "")", .secondaryLabelColor)
            line(r["curl"] as? String ?? "")
        }
        if let err = e["lastError"] as? String, !err.isEmpty {
            head("\nLAST ERROR")
            line(err, .systemRed)
            if let c = e["lastCurl"] as? String, !c.isEmpty {
                line("# the failing request ($JIRA_TOKEN = your token)", .secondaryLabelColor)
                line(c)
            }
        }
        for n in e["notes"] as? [String] ?? [] { line("ℹ︎ \(n)", .secondaryLabelColor) }
        let keepScroll = detail.enclosingScrollView?.contentView.bounds.origin
        detail.textStorage?.setAttributedString(out)
        if let o = keepScroll { detail.enclosingScrollView?.contentView.scroll(to: o) }
    }

    private func updateColsNote() {
        let tabs = eps.compactMap { $0["file"] as? String }.joined(separator: ", ")
        let api = (info["apiFields"] as? [String] ?? []).joined(separator: ",")
        colsNote.stringValue = (colsDirty ? "● unsaved changes — " : "")
            + "Applies to every tab: \(tabs.isEmpty ? "(none)" : tabs)\nAPI fields= \(api)"
        saveColsButton.isEnabled = colsDirty
        revertColsButton.isEnabled = colsDirty
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

    // MARK: table data

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === jobs ? eps.count : cols.count
    }

    private func label(_ s: String, color: NSColor = .labelColor, mono: Bool = false) -> NSTextField {
        let f = NSTextField(labelWithString: s)
        f.lineBreakMode = .byTruncatingTail
        f.textColor = color
        if mono { f.font = .monospacedSystemFont(ofSize: 11, weight: .regular) }
        f.toolTip = s
        return f
    }

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
        guard let id = tableColumn?.identifier.rawValue else { return nil }
        return tableView === jobs ? jobCell(id, row) : colCell(id, row)
    }

    private func jobCell(_ id: String, _ row: Int) -> NSView? {
        guard row < eps.count else { return nil }
        let e = eps[row]
        let name = e["name"] as? String ?? ""
        switch id {
        case "on":
            let b = NSButton(checkboxWithTitle: "", target: self, action: #selector(toggleJob(_:)))
            b.state = (e["enabled"] as? Bool ?? true) ? .on : .off
            b.tag = row
            b.toolTip = "Include \(name) in scheduled polls"
            return cell(b)
        case "window":
            let p = NSPopUpButton(frame: .zero, pullsDown: false)
            p.controlSize = .small
            let cur = e["window"] as? String ?? ""
            var choices = JiraPoll.intervals
            if !cur.isEmpty && !choices.contains(cur) { choices.insert(cur, at: 0) }
            p.addItems(withTitles: choices)
            p.selectItem(withTitle: cur)
            p.tag = row
            p.target = self
            p.action = #selector(setInterval(_:))
            return cell(p)
        case "status":
            var st = e["status"] as? String ?? ""
            if JiraPoll.running.contains(name) || JiraPoll.running.contains("all") { st = "running" }
            let color: NSColor = st == "ok" ? .systemGreen : st == "error" ? .systemRed
                : st == "running" ? .systemOrange : .secondaryLabelColor
            let l = label(st, color: color)
            if let err = e["lastError"] as? String, !err.isEmpty { l.toolTip = err }
            return cell(l)
        case "lastRun", "nextRun":
            return cell(label(JiraPoll.short(e[id] as? String)))
        case "items":
            return cell(label((e["items"] as? Int).map(String.init) ?? "–"))
        case "nextWindow":
            return cell(label(e["nextWindow"] as? String ?? "", mono: true))
        default:
            return cell(label(e[id] as? String ?? ""))
        }
    }

    private func colCell(_ id: String, _ row: Int) -> NSView? {
        guard row < cols.count else { return nil }
        let c = cols[row]
        func edit(_ s: String, _ tag: String) -> NSView {
            let f = NSTextField(string: s)
            f.isBordered = false
            f.drawsBackground = false
            f.identifier = NSUserInterfaceItemIdentifier(tag)
            f.tag = row
            f.delegate = self
            f.target = self
            f.action = #selector(colFieldEdited(_:))
            return cell(f)
        }
        func check(_ on: Bool, _ tag: String) -> NSView {
            let b = NSButton(checkboxWithTitle: "", target: self, action: #selector(colFlagToggled(_:)))
            b.state = on ? .on : .off
            b.identifier = NSUserInterfaceItemIdentifier(tag)
            b.tag = row
            return cell(b)
        }
        let meta = colMeta[c.field] ?? [:]
        switch id {
        case "field": return cell(label(c.field, mono: true))
        case "title": return edit(c.title, "title")
        case "width":
            return edit(c.width == c.width.rounded() ? String(Int(c.width)) : String(format: "%.1f", c.width), "width")
        case "align":
            let p = NSPopUpButton(frame: .zero, pullsDown: false)
            p.controlSize = .small
            p.addItems(withTitles: ["left", "center", "right"])
            p.selectItem(withTitle: c.align)
            p.tag = row
            p.target = self
            p.action = #selector(colAlignChanged(_:))
            return cell(p)
        case "sort": return check(c.sortable, "sort")
        case "filter": return check(c.filterable, "filter")
        case "api":
            let api = (meta["apiFields"] as? [String])?.joined(separator: ", ")
            return cell(label(api ?? (colsDirty ? "(save to resolve)" : c.field), color: .secondaryLabelColor, mono: true))
        case "label":
            let l = meta["label"] as? String ?? ""
            let d = meta["description"] as? String ?? ""
            let v = label(l.isEmpty ? "" : l, color: .secondaryLabelColor)
            v.toolTip = d.isEmpty ? l : "\(l) — \(d)"
            return cell(v)
        default: return nil
        }
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        if (notification.object as? NSTableView) === jobs { updateDetail() }
    }

    // MARK: job actions

    @objc private func toggleJob(_ sender: NSButton) {
        guard sender.tag < eps.count, let n = eps[sender.tag]["name"] as? String else { return }
        let on = sender.state == .on
        JiraPoll.run("jira_config.py", ["--set-enabled", n, on ? "true" : "false"]) { [weak self] code, _, err in
            self?.controller?.log("jira: \(n) enabled -> \(on) (exit \(code))\(code == 0 ? "" : " " + err)")
            self?.refresh()
        }
    }

    @objc private func setInterval(_ sender: NSPopUpButton) {
        guard sender.tag < eps.count, let n = eps[sender.tag]["name"] as? String,
              let w = sender.titleOfSelectedItem else { return }
        JiraPoll.run("jira_config.py", ["--set-window", n, w]) { [weak self] code, _, err in
            self?.controller?.log("jira: \(n) window -> \(w) (exit \(code))\(code == 0 ? "" : " " + err)")
            self?.refresh()
        }
    }

    private func poll(_ endpoint: String, full: Bool) {
        controller?.jiraPollNow(endpoint, full: full) { [weak self] in self?.refresh() }
        refresh()
    }

    @objc private func pollAll(_ sender: Any?) { poll("all", full: false) }
    @objc private func pollSelected(_ sender: Any?) {
        if let n = selected?["name"] as? String { poll(n, full: false) }
    }
    @objc private func resyncSelected(_ sender: Any?) {
        if let n = selected?["name"] as? String { poll(n, full: true) }
    }

    @objc private func cancelPoll(_ sender: Any?) {
        JiraPoll.run("jira_poll.py", ["--cancel"]) { [weak self] code, out, err in
            self?.controller?.log("jira: cancel -> \(out.trimmingCharacters(in: .whitespacesAndNewlines))"
                                  + (code == 0 ? "" : " " + err))
            self?.refresh()
        }
    }

    private func toPasteboard(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    @objc private func copyJQL(_ sender: Any?) {
        if let j = selected?["jql"] as? String, !j.isEmpty { toPasteboard(j) }
    }

    @objc private func copyCurl(_ sender: Any?) {
        let reqs = selected?["requests"] as? [[String: Any]] ?? []
        let text = reqs.map { "# \($0["purpose"] as? String ?? "")\n\($0["curl"] as? String ?? "")" }
            .joined(separator: "\n")
        if !text.isEmpty { toPasteboard(text) }
    }

    @objc private func toggleEnabled(_ sender: Any?) {
        controller?.toggleJiraPoll()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.refresh() }
    }

    @objc private func setup(_ sender: Any?) { controller?.showJiraSetup() }

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

    // MARK: connection actions

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
        if let c = info["loginCurl"] as? String, !c.isEmpty {
            toPasteboard(c)
            connResult.stringValue = "Login curl copied (includes the token)."
            connResult.textColor = .secondaryLabelColor
        }
    }

    // MARK: column editing

    private func markColsDirty() {
        colsDirty = true
        updateColsNote()
    }

    @objc private func colFieldEdited(_ sender: NSTextField) {
        let r = sender.tag
        guard r < cols.count else { return }
        let v = sender.stringValue.trimmingCharacters(in: .whitespaces)
        switch sender.identifier?.rawValue {
        case "title":
            // ':' and ',' are the columns-line separators
            let t = v.replacingOccurrences(of: ":", with: " ").replacingOccurrences(of: ",", with: " ")
            guard t != cols[r].title else { return }
            cols[r].title = t.isEmpty ? cols[r].field : t
        case "width":
            guard let w = Double(v), w >= 0, w <= 100 else { sender.stringValue = String(Int(cols[r].width)); return }
            guard CGFloat(w) != cols[r].width else { return }
            cols[r].width = CGFloat(w)
        default: return
        }
        markColsDirty()
    }

    func controlTextDidEndEditing(_ obj: Notification) {
        if let f = obj.object as? NSTextField, f.identifier != nil { colFieldEdited(f) }
    }

    @objc private func colFlagToggled(_ sender: NSButton) {
        guard sender.tag < cols.count else { return }
        if sender.identifier?.rawValue == "sort" { cols[sender.tag].sortable = sender.state == .on }
        else { cols[sender.tag].filterable = sender.state == .on }
        markColsDirty()
    }

    @objc private func colAlignChanged(_ sender: NSPopUpButton) {
        guard sender.tag < cols.count, let a = sender.titleOfSelectedItem else { return }
        cols[sender.tag].align = a
        markColsDirty()
    }

    @objc private func addColumn(_ sender: Any?) {
        let f = addField.stringValue.trimmingCharacters(in: .whitespaces)
            .replacingOccurrences(of: ":", with: "").replacingOccurrences(of: ",", with: "")
        guard !f.isEmpty, !cols.contains(where: { $0.field == f }) else { return }
        let title = (colMeta[f]?["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? f
        cols.append(ListColumn(field: f, title: title, width: 0, align: "left", sortable: true, filterable: true))
        addField.stringValue = ""
        colTable.reloadData()
        colTable.selectRowIndexes(IndexSet(integer: cols.count - 1), byExtendingSelection: false)
        colTable.scrollRowToVisible(cols.count - 1)
        markColsDirty()
    }

    @objc private func removeColumn(_ sender: Any?) {
        let r = colTable.selectedRow
        guard r >= 0, r < cols.count, cols.count > 1 else { return }
        cols.remove(at: r)
        colTable.reloadData()
        markColsDirty()
    }

    private func move(_ d: Int) {
        let r = colTable.selectedRow
        let to = r + d
        guard r >= 0, to >= 0, to < cols.count else { return }
        cols.swapAt(r, to)
        colTable.reloadData()
        colTable.selectRowIndexes(IndexSet(integer: to), byExtendingSelection: false)
        markColsDirty()
    }

    @objc private func moveUp(_ sender: Any?) { move(-1) }
    @objc private func moveDown(_ sender: Any?) { move(1) }

    @objc private func revertColumns(_ sender: Any?) {
        colsDirty = false
        cols = ListColumn.parse(jiraConfigValue("columns"))
        colTable.reloadData()
        updateColsNote()
    }

    @objc private func saveColumns(_ sender: Any?) {
        window.makeFirstResponder(nil)   // commit an in-progress cell edit
        let spec = ListColumn.serialize(cols)
        controller?.saveJiraColumns(cols, spec: spec)
        colsDirty = false
        updateColsNote()
        refresh()
    }

    // MARK: close

    private func close() {
        window.orderOut(nil)
        teardown()
    }

    private func teardown() {
        timer?.invalidate()
        timer = nil
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        JiraDashboardWindow.live = nil
    }

    func windowWillClose(_ notification: Notification) { teardown() }
}
