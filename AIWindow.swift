import AppKit
import WebKit

// MARK: - AI view (a shared-window view)
//
// Apple's on-device model through the `fm` CLI, driven by rule files: every
// .md in [ai] rules-dir is a pill. Rule format: see rules/grammar-check.md.
// Config: commands.toml [ai]. Open from the Hyper+S palette (/ai).

// [ai] enabled - the view, its hotkey and its menu entry
func aiEnabled() -> Bool {
    tri(configSectionValue("ai", "enabled")) == true
}

func aiSetting(_ key: String, _ fallback: String) -> String {
    let v = configSectionValue("ai", key)?.trimmingCharacters(in: .whitespaces) ?? ""
    return v.isEmpty ? fallback : v
}

func aiNumber(_ key: String, _ fallback: CGFloat) -> CGFloat {
    configSectionValue("ai", key).flatMap { Double($0) }.map { CGFloat($0) } ?? fallback
}

private func aiPath(_ key: String, _ fallback: String) -> String {
    (aiSetting(key, fallback) as NSString).expandingTildeInPath
}

// (rule files: AIRule in AIFormat.swift)

// (the word diff: CharDiff in CompareText.swift)

// MARK: - text view with a placeholder

final class AITextView: NSTextView {
    var placeholder = "" { didSet { needsDisplay = true } }
    var placeholderColor = NSColor.secondaryLabelColor
    override func draw(_ dirty: NSRect) {
        super.draw(dirty)
        guard string.isEmpty, !placeholder.isEmpty else { return }
        let attrs: [NSAttributedString.Key: Any] = [.font: font ?? .systemFont(ofSize: 14),
                                                    .foregroundColor: placeholderColor]
        let inset = textContainerInset
        let pad = textContainer?.lineFragmentPadding ?? 5
        (placeholder as NSString).draw(in: NSRect(x: inset.width + pad, y: inset.height,
                                                  width: bounds.width - inset.width * 2 - pad * 2,
                                                  height: bounds.height), withAttributes: attrs)
    }
    override func didChangeText() {
        super.didChangeText()
        needsDisplay = true
    }
}

// the command line under the pills: click copies a runnable version
final class AICommandLine: NSView, PopupThemeable {
    var colors = JiraTheme.system { didSet { needsDisplay = true } }
    var text = "" { didSet { needsDisplay = true } }
    var warning = "" { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?
    override var isFlipped: Bool { true }
    func applyColors(_ c: PopupColors) { colors = c }
    override func draw(_ dirty: NSRect) {
        let font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        let p = NSMutableParagraphStyle()
        p.lineBreakMode = .byTruncatingMiddle
        let s = NSMutableAttributedString(string: "$ ", attributes: [.font: font, .foregroundColor: colors.accent])
        s.append(NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: colors.dim]))
        if !warning.isEmpty {
            s.append(NSAttributedString(string: "   ⚠ " + warning,
                                        attributes: [.font: font, .foregroundColor: colors.tone(.warning)]))
        }
        s.addAttribute(.paragraphStyle, value: p, range: NSRange(location: 0, length: s.length))
        let h = s.size().height
        s.draw(in: NSRect(x: 14, y: (bounds.height - h) / 2, width: bounds.width - 28, height: h))
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }
    override func mouseDown(with event: NSEvent) { onClick?() }
}

// the steps of a run (the rule, then each `then:`), one chip each:
// waiting → running → done / warning / failed
final class AIPipelineStrip: NSView, PopupThemeable {
    enum State { case waiting, running, done, warn, failed }
    var colors = JiraTheme.system { didSet { needsDisplay = true } }
    private(set) var items: [(name: String, state: State, note: String)] = []
    override var isFlipped: Bool { true }
    func applyColors(_ c: PopupColors) { colors = c }

    func set(_ names: [String]) {
        items = names.map { ($0, .waiting, "") }
        needsDisplay = true
    }
    func mark(_ i: Int, _ state: State, note: String = "") {
        guard items.indices.contains(i) else { return }
        items[i].state = state
        items[i].note = note
        toolTip = items.compactMap { $0.note.isEmpty ? nil : "\($0.name): \($0.note)" }.joined(separator: "\n")
        needsDisplay = true
    }
    /// everything before `i` is done, `i` is running, the rest waits
    func running(_ i: Int) {
        for k in items.indices {
            if k < i { if items[k].state == .waiting || items[k].state == .running { items[k].state = .done } }
            else { items[k].state = k == i ? .running : .waiting; items[k].note = "" }
        }
        needsDisplay = true
    }

    override func draw(_ dirty: NSRect) {
        let font = NSFont.systemFont(ofSize: 11.5, weight: .medium)
        var x: CGFloat = 2
        let h: CGFloat = 22, y = (bounds.height - h) / 2
        for (i, it) in items.enumerated() {
            let (glyph, tone): (String, NSColor) = {
                switch it.state {
                case .waiting: return ("\(i + 1)", colors.dim)
                case .running: return ("…", colors.accent)
                case .done: return ("✓", colors.tone(.success))
                case .warn: return ("!", colors.tone(.warning))
                case .failed: return ("✕", colors.tone(.danger))
                }
            }()
            let label = NSAttributedString(string: "\(glyph)  \(it.name)", attributes: [.font: font, .foregroundColor: tone])
            let w = label.size().width + 20
            if x + w > bounds.width { break }
            let r = NSRect(x: x, y: y, width: w, height: h)
            let path = NSBezierPath(roundedRect: r, xRadius: h / 2, yRadius: h / 2)
            tone.withAlphaComponent(it.state == .waiting ? 0.08 : 0.16).setFill()
            path.fill()
            label.draw(at: NSPoint(x: r.minX + 10, y: r.midY - label.size().height / 2))
            x = r.maxX
            if i + 1 < items.count {
                let arrow = NSAttributedString(string: "→", attributes: [.font: font, .foregroundColor: colors.dim])
                arrow.draw(at: NSPoint(x: x + 6, y: y + (h - arrow.size().height) / 2))
                x += 12 + arrow.size().width
            }
        }
    }
}

// MARK: - the window

final class AIWindow: CardWindowController, NSTextViewDelegate, WKNavigationDelegate {
    private static var live: AIWindow?
    static var current: AIWindow? { live }

    private var colors = cardColors("ai")

    private var body: ConfPane!
    private var pills: PopupTabsBar!
    private let cmdLine = AICommandLine()
    private let strip = AIPipelineStrip()
    private let leftBox = ConfPane(), rightBox = ConfPane()
    private let leftTitle = NSTextField(labelWithString: "Your text")
    private let rightTitle = NSTextField(labelWithString: "Output")
    private let input = AITextView()
    private let output = AITextView()
    private let inScroll = NSScrollView(), outScroll = NSScrollView()
    private var web: WKWebView!
    private let modeSeg = ConfSegmented(["Diff", "Outlook", "Webex"])
    private let copyButton = ThemedPushButton(title: "⧉ Copy", target: nil, action: nil)
    private let runButton = ThemedPushButton(title: "Run  ⌃↩", target: nil, action: nil)
    private let status = NSTextField(labelWithString: "")
    private let tokens = NSTextField(labelWithString: "")
    private let spinner = NSProgressIndicator()
    private let splitter = PaneSplitter()
    private var split: CGFloat = 0.5
    private var sidebarW: CGFloat = 0
    private var toast: NSView?

    // state
    private var rules: [AIRule] = []
    private var selected = 0
    private var rule: AIRule? { rules.indices.contains(selected) ? rules[selected] : nil }
    private var answer = ""             // the clean output of the last run (Markdown)
    private var answerInput = ""        // the input it answered
    private var answerDiff = false
    private var mode = PaneMode.diff    // the right pane (what Copy writes follows the last preview)
    private var modes: [PaneMode] { PaneMode.all(diff: rule?.diff ?? true) }
    private var target = PasteTarget.outlook    // what Copy writes (the last preview picked)
    private var rendered: [Int: String] = [:]   // target -> styled HTML of `answer`
    private var renderGen = 0
    private var process: Process?
    private var runGen = 0
    private var started = Date()
    private var available: Bool?        // `fm available`
    private var unavailableWhy = ""
    private var dirWatch: DispatchSourceFileSystemObject?
    // one run = its parts (long text), run one after another
    private var guardCode: CodeGuard?
    private var parts: [String] = []
    private var partIndex = 0
    private var doneParts: [String] = []
    private var streamData = Data()
    // a run's rules: the picked one, then each `then:` on the answer before
    private var steps: [AIRule] = []
    private var stepIndex = 0
    private var stepInput = ""          // what the current step was given
    private var stepNotes: [String] = []
    private var tokenTimer: Timer?
    private var tokenGen = 0

    private static let ruleKey = "aiRule"
    private static let splitKey = "aiSplit"
    private static let modeKey = "aiViewMode"
    private static let targetKey = "aiTarget"
    private static func inputKey(_ file: String) -> String { "aiInput." + file }

    private var fmBin: String { aiPath("fm-bin", "/usr/bin/fm") }
    private var rulesDir: String { aiPath("rules-dir", userDir + "/rules") }
    private var fontSize: CGFloat { max(9, min(32, aiNumber("font-size", 14))) }
    private var textFont: NSFont {
        let name = aiSetting("font", "")
        return (name.isEmpty ? nil : NSFont(name: name, size: fontSize)) ?? .systemFont(ofSize: fontSize)
    }

    // MARK: open

    // a theme rebuild keeps what was typed
    private static var carriedInput: String?
    static func discard() { carriedInput = live?.input.string; live = nil }

    static func create(controller: SwitcherController, frame: NSRect?) {
        guard live == nil else { return }
        live = AIWindow(controller: controller, frame: frame)
    }

    // [app] shared-window = false: an ordinary window
    func showStandalone() {
        NSApp.activate(ignoringOtherApps: true)
        if !window.isVisible { window.center() }
        window.makeKeyAndOrderFront(nil)
        reloadRules()
        focusInput()
    }

    // the shared window brought the view back
    override func didShow() {
        reloadRules()
        if window.firstResponder === window || window.firstResponder == nil { focusInput() }
    }

    private init(controller: SwitcherController, frame: NSRect?) {
        let f = frame ?? NSRect(x: 0, y: 0, width: aiNumber("width", 900), height: aiNumber("height", 600))
        super.init(controller: controller, frame: f, title: "AI", minSize: NSSize(width: 560, height: 360))
        split = CGFloat(UserDefaults.standard.double(forKey: Self.splitKey))
        if split < 0.2 || split > 0.8 { split = aiNumber("split", 0.5) }
        mode = PaneMode(rawValue: UserDefaults.standard.string(forKey: Self.modeKey) ?? "") ?? .diff
        target = PasteTarget(rawValue: UserDefaults.standard.integer(forKey: Self.targetKey)) ?? .outlook
        PopupThemeDefaults.colors = colors
        window.contentView = themedRoot(buildContent(), name: "ai", colors: colors,
                                        headerColor: cardHeaderColor("ai"),
                                        icon: aiAppIcon, title: "AI")
        if frame != nil { window.setFrame(f, display: false) }
        reloadRules()
        watchRulesDir()
        checkAvailable()
        setMode(mode)
        if let t = Self.carriedInput { input.string = t; Self.carriedInput = nil }
    }

    // MARK: build

    private func label(_ f: NSTextField, size: CGFloat = 11, weight: NSFont.Weight = .regular, color: NSColor? = nil) {
        f.font = .systemFont(ofSize: size, weight: weight)
        f.textColor = color ?? colors.dim
        f.lineBreakMode = .byTruncatingTail
        f.maximumNumberOfLines = 1
        f.cell?.truncatesLastVisibleLine = true
    }

    private func hook(_ b: NSButton, _ sel: Selector, tip: String? = nil) {
        b.target = self
        b.action = sel
        b.toolTip = tip
        b.controlSize = .small
    }

    private func setupText(_ tv: AITextView, _ scroll: NSScrollView, editable: Bool) {
        tv.isEditable = editable
        tv.isSelectable = true
        tv.isRichText = false
        tv.allowsUndo = editable
        tv.importsGraphics = false
        tv.drawsBackground = false
        tv.font = textFont
        tv.textColor = colors.text
        tv.insertionPointColor = colors.accent
        tv.placeholderColor = colors.dim.withAlphaComponent(0.7)
        tv.selectedTextAttributes = [.backgroundColor: colors.highlight]
        tv.textContainerInset = NSSize(width: 8, height: 8)
        tv.isAutomaticQuoteSubstitutionEnabled = false
        tv.isAutomaticDashSubstitutionEnabled = false
        tv.isAutomaticTextReplacementEnabled = false
        tv.isAutomaticSpellingCorrectionEnabled = false
        tv.isContinuousSpellCheckingEnabled = false
        tv.isGrammarCheckingEnabled = false
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.autoresizingMask = [.width]
        tv.textContainer?.widthTracksTextView = true
        tv.minSize = .zero
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        scroll.documentView = tv
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
    }

    private func buildContent() -> NSView {
        let root = ConfPane()
        body = root
        var pcfg = PopupConfig(name: "ai-rules")
        pcfg.colors = colors
        pcfg.tabsAddButton = true
        pills = PopupTabsBar(config: pcfg)
        pills.closable = false
        pills.onSelect = { [weak self] i in self?.select(i) }
        pills.onAddTab = { [weak self] in self?.newRule() }
        pills.menuFor = { [weak self] i in self?.pillMenu(i) }
        // the rules as a sidebar (`[ai] sidebar-width`, 0 = a strip); its
        // edge drags to resize, the width is saved on release
        sidebarW = aiNumber("sidebar-width", 210)
        if sidebarW > 0 {
            pills.vertical = true
            pills.sectionTitle = "Rules"
            pills.rowIcon = { _ in "wand.and.stars" }
            pills.pathTip = { [weak self] i in
                guard let self, self.rules.indices.contains(i) else { return nil }
                return (self.rules[i].path as NSString).abbreviatingWithTildeInPath
            }
            pills.onWidthChange = { [weak self] w, done in
                guard let self else { return }
                self.sidebarW = w.rounded()
                self.body?.needsLayout = true
                if done { saveConfigValue(section: "ai", key: "sidebar-width", value: String(Int(self.sidebarW))) }
            }
        } else {
            pills.toolTip = "Rules — one .md file each (right-click: edit, reveal, duplicate, delete)"
        }
        root.addSubview(pills)

        cmdLine.onClick = { [weak self] in self?.copyCommand() }
        cmdLine.toolTip = "The command this rule runs — click to copy it (with your text) for a shell"
        root.addSubview(cmdLine)
        strip.toolTip = ""
        root.addSubview(strip)

        for (box, title) in [(leftBox, leftTitle), (rightBox, rightTitle)] {
            box.fill = colors.mantle.withAlphaComponent(0.45)
            box.wantsLayer = true
            box.layer?.cornerRadius = 8
            box.layer?.borderWidth = 1
            box.layer?.borderColor = colors.hairline.cgColor
            label(title, size: 11, weight: .semibold)
            box.addSubview(title)
            root.addSubview(box)
        }
        setupText(input, inScroll, editable: true)
        input.delegate = self
        leftBox.addSubview(inScroll)
        setupText(output, outScroll, editable: false)
        output.placeholder = "The answer shows up here."
        rightBox.addSubview(outScroll)

        // the paste preview: the same HTML Copy writes, on a mock surface
        let wc = WKWebViewConfiguration()
        web = QuietWebView(frame: .zero, configuration: wc)
        web.navigationDelegate = self
        web.setValue(false, forKey: "drawsBackground")
        web.isHidden = true
        rightBox.addSubview(web)

        modeSeg.tips = ["Your text with the model's changes marked", "The answer as Markdown",
                        "How it looks pasted into Outlook — Copy writes this",
                        "How it looks pasted into Webex — tables become aligned text"]
        modeSeg.onChange = { [weak self] i in
            guard let self, self.modes.indices.contains(i) else { return }
            self.setMode(self.modes[i])
        }
        hook(copyButton, #selector(copyAnswer), tip: "Copy as rich text for the target")
        for v in [modeSeg, copyButton] as [NSView] { rightBox.addSubview(v) }

        hook(runButton, #selector(runClicked), tip: "Run the rule on your text (Ctrl+Enter)")
        runButton.role = .primary
        runButton.controlSize = .regular
        label(status, size: 11.5)
        label(tokens, size: 11)
        tokens.alignment = .right
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        spinner.isHidden = true
        for v in [runButton, spinner, status, tokens] as [NSView] { root.addSubview(v) }

        splitter.onFractionChange = { [weak self] f in
            guard let self else { return }
            var f = f
            if self.pills.vertical {
                // the splitter measures across the whole view; the panes
                // start right of the sidebar
                let full = root.bounds.width, left = self.sidebarW + 4 + 12
                let avail = max(1, full - self.sidebarW - 4 - 24 - 10)
                f = min(0.8, max(0.2, (f * full - left) / avail))
            }
            self.split = f
            UserDefaults.standard.set(Double(f), forKey: Self.splitKey)
            root.needsLayout = true
        }
        root.addSubview(splitter)
        root.onLayout = { [weak self] b in self?.layoutAll(b) }
        return root
    }

    private func layoutAll(_ full: NSRect) {
        let pad: CGFloat = 12
        // sidebar: the rules down the left, everything else beside it
        var b = full
        let cmdH: CGFloat = 22
        if pills.vertical {
            pills.frame = NSRect(x: 0, y: 0, width: sidebarW, height: full.height)
            b = NSRect(x: 0, y: 0, width: max(200, full.width - sidebarW - 4), height: full.height)
            cmdLine.frame = NSRect(x: sidebarW + 4, y: 6, width: b.width, height: cmdH)
        } else {
            let pillsH = pills.heightNeeded(forWidth: b.width) + 6
            pills.frame = NSRect(x: 0, y: 4, width: b.width, height: pillsH)
            cmdLine.frame = NSRect(x: 0, y: pills.frame.maxY, width: b.width, height: cmdH)
        }
        defer {
            // shift the content right of the sidebar
            if pills.vertical {
                for v in [strip, leftBox, rightBox, splitter, runButton, spinner, tokens, status] as [NSView] {
                    v.frame.origin.x += sidebarW + 4
                }
            }
        }
        let footH: CGFloat = 44
        strip.frame = NSRect(x: pad, y: cmdLine.frame.maxY + 4, width: max(0, b.width - pad * 2), height: 28)
        let top = strip.frame.maxY + 4
        let bodyH = max(80, b.height - top - footH)
        let gap: CGFloat = 10
        let avail = b.width - pad * 2 - gap
        let leftW = (avail * split).rounded()
        leftBox.frame = NSRect(x: pad, y: top, width: leftW, height: bodyH)
        rightBox.frame = NSRect(x: pad + leftW + gap, y: top, width: avail - leftW, height: bodyH)
        splitter.frame = NSRect(x: leftBox.frame.maxX, y: top, width: gap, height: bodyH)
        let headH: CGFloat = 32
        for (box, title, scroll) in [(leftBox, leftTitle, inScroll), (rightBox, rightTitle, outScroll)] {
            title.sizeToFit()
            title.frame = NSRect(x: 12, y: (headH - title.frame.height) / 2 + 1,
                                 width: min(title.frame.width + 4, box.bounds.width / 3), height: title.frame.height)
            scroll.frame = NSRect(x: 2, y: headH, width: box.bounds.width - 4, height: box.bounds.height - headH - 2)
        }
        web.frame = outScroll.frame
        // output header, right-aligned: Diff | Outlook | Webex · ⧉ Copy
        copyButton.sizeToFit()
        let cw = copyButton.frame.width + 8
        var x = rightBox.bounds.width - 8 - cw
        copyButton.frame = NSRect(x: x, y: (headH - 24) / 2, width: cw, height: 24)
        let sw = modeSeg.intrinsicContentSize.width
        x -= 6 + sw
        modeSeg.frame = NSRect(x: x, y: (headH - JiraTheme.height) / 2, width: sw, height: JiraTheme.height)
        // squeezed: the title ("Changes") gives way to the buttons, never under them
        let room = x - 8 - rightTitle.frame.minX
        rightTitle.isHidden = room < rightTitle.frame.width
        // footer: Run · spinner · status ……… tokens
        runButton.sizeToFit()
        let fy = top + bodyH + (footH - 28) / 2
        runButton.frame = NSRect(x: pad, y: fy, width: runButton.frame.width + 16, height: 28)
        spinner.frame = NSRect(x: runButton.frame.maxX + 10, y: fy + 6, width: 16, height: 16)
        tokens.sizeToFit()
        let tw = min(tokens.frame.width + 4, b.width * 0.4)
        tokens.frame = NSRect(x: b.width - pad - tw, y: fy + 6, width: tw, height: 16)
        let sx = runButton.frame.maxX + (spinner.isHidden ? 10 : 32)
        status.frame = NSRect(x: sx, y: fy + 6, width: max(0, tokens.frame.minX - sx - 10), height: 16)
    }

    // MARK: rules

    private func ensureRulesDir() {
        guard !FileManager.default.fileExists(atPath: rulesDir) else { return }
        try? FileManager.default.createDirectory(atPath: rulesDir, withIntermediateDirectories: true)
    }

    private func reloadRules() {
        ensureRulesDir()
        let dir = rulesDir
        let names = ((try? FileManager.default.contentsOfDirectory(atPath: dir)) ?? [])
            .filter { !$0.hasPrefix(".") && $0.lowercased().hasSuffix(".md") }
            .sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        let was = rule?.file
        let keep = was ?? UserDefaults.standard.string(forKey: Self.ruleKey)
        rules = names.map { AIRule.load(dir + "/" + $0) }
        selected = rules.firstIndex { $0.file == keep } ?? 0
        pills.titles = rules.map(\.name)
        pills.selected = selected
        body.needsLayout = true
        // a different rule than before (first load, the old one deleted):
        // its own saved text
        applyRule(loadInput: rule?.file != was)
    }

    // the directory changes (a rule added, removed, saved by vim's rename)
    private func watchRulesDir() {
        let fd = open(rulesDir, O_EVTONLY)
        guard fd >= 0 else { return }
        let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .rename, .delete],
                                                            queue: .main)
        src.setEventHandler { [weak self] in
            // let an editor's write-then-rename settle
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self?.reloadRules() }
        }
        src.setCancelHandler { close(fd) }
        src.resume()
        dirWatch = src
    }

    private func select(_ i: Int) {
        guard rules.indices.contains(i) else { return }
        saveInput()
        selected = i
        pills.selected = i
        UserDefaults.standard.set(rules[i].file, forKey: Self.ruleKey)
        applyRule(loadInput: true)
        focusInput()
    }

    private func applyRule(loadInput: Bool) {
        guard let r = rule else {
            cmdLine.text = "no rules in \(rulesDir) — press + to make one"
            cmdLine.warning = ""
            return
        }
        cmdLine.text = r.preview
        cmdLine.warning = r.warnings.joined(separator: ", ")
        if process == nil { strip.set(AIRule.chain(r).map(\.name)) }
        input.placeholder = r.placeholder.isEmpty ? "Type or paste your text — Ctrl+Enter runs it" : r.placeholder
        modeSeg.items = modes.map(\.title)
        if !modes.contains(mode) { mode = .markdown }
        modeSeg.selected = modes.firstIndex(of: mode) ?? 0
        rightTitle.stringValue = r.diff ? "Changes" : "Answer"
        scheduleTokenCount()
        if loadInput {
            input.string = UserDefaults.standard.string(forKey: Self.inputKey(r.file)) ?? ""
            input.needsDisplay = true
            clearAnswer()
        }
        body.needsLayout = true
    }

    private func saveInput() {
        guard let r = rule else { return }
        UserDefaults.standard.set(input.string, forKey: Self.inputKey(r.file))
    }

    func textDidChange(_ notification: Notification) {
        guard (notification.object as? NSTextView) === input else { return }
        saveInput()
        scheduleTokenCount()
    }

    private func pillMenu(_ i: Int) -> NSMenu? {
        guard rules.indices.contains(i) else { return nil }
        let r = rules[i]
        let menu = NSMenu()
        func add(_ t: String, _ f: @escaping () -> Void) { menu.addItem(menuItem(t, f)) }
        add("Edit in Notes") { [weak self] in self?.controller?.openNoteFile(r.path) }
        add("Open in Default App") { NSWorkspace.shared.open(URL(fileURLWithPath: r.path)) }
        add("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: r.path)]) }
        add("Copy Path") { [weak self] in self?.copy(r.path, what: "path") }
        menu.addItem(.separator())
        add("Duplicate…") { [weak self] in self?.newRule(from: r) }
        add("Delete…") { [weak self] in self?.deleteRule(r) }
        return menu
    }

    private func slug(_ name: String) -> String {
        let s = name.lowercased().replacingOccurrences(of: #"[^a-z0-9]+"#, with: "-", options: .regularExpression)
        return s.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    }

    // "+" (or Duplicate): name it, write rules/<slug>.md, edit it in notes
    private func newRule(from source: AIRule? = nil) {
        let field = NSTextField(string: source.map { $0.name + " copy" } ?? "")
        field.placeholderString = "Summarize, Rewrite formally, Explain…"
        jiraFormSheet(on: window, title: source == nil ? "New Rule" : "Duplicate Rule",
                      info: "A rule is a .md file in \(rulesDir.replacingOccurrences(of: NSHomeDirectory(), with: "~")): "
                          + "frontmatter for the fm options, then the instructions.",
                      rows: [("Name", field)], ok: "Create") { [weak self] ok in
            guard let self, ok else { return }
            let name = field.stringValue.trimmingCharacters(in: .whitespaces)
            let base = self.slug(name)
            guard !base.isEmpty else { return }
            var path = self.rulesDir + "/" + base + ".md"
            var n = 2
            while FileManager.default.fileExists(atPath: path) { path = self.rulesDir + "/\(base)-\(n).md"; n += 1 }
            var text: String
            if let src = source, let body = try? String(contentsOfFile: src.path, encoding: .utf8) {
                text = body.replacingOccurrences(of: #"(?m)^name:.*$"#, with: "name: " + name,
                                                 options: .regularExpression)
            } else {
                text = Self.ruleTemplate.replacingOccurrences(of: "{name}", with: name)
            }
            do { try text.write(toFile: path, atomically: true, encoding: .utf8) } catch {
                self.setStatus("Couldn't write \(path): \(error.localizedDescription)", tone: .danger)
                return
            }
            UserDefaults.standard.set((path as NSString).lastPathComponent, forKey: Self.ruleKey)
            self.reloadRules()
            if let i = self.rules.firstIndex(where: { $0.path == path }) { self.select(i) }
            self.controller?.openNoteFile(path)
        }
    }

    private func deleteRule(_ r: AIRule) {
        let a = NSAlert()
        a.messageText = "Delete “\(r.name)”?"
        a.informativeText = "\(r.file) goes to the Trash."
        a.addButton(withTitle: "Move to Trash")
        a.addButton(withTitle: "Cancel")
        a.beginSheetModal(for: window) { [weak self] resp in
            guard resp == .alertFirstButtonReturn else { return }
            try? FileManager.default.trashItem(at: URL(fileURLWithPath: r.path), resultingItemURL: nil)
            UserDefaults.standard.removeObject(forKey: Self.inputKey(r.file))
            self?.reloadRules()
        }
    }

    // MARK: run

    private func checkAvailable() {
        let bin = fmBin
        guard FileManager.default.isExecutableFile(atPath: bin) else {
            available = false
            unavailableWhy = "fm not found at \(bin) ([ai] fm-bin) — it ships with macOS 26+"
            setStatus(unavailableWhy, tone: .danger)
            return
        }
        DispatchQueue.global(qos: .userInitiated).async {
            let (code, text) = Self.capture(bin, ["available"], stdin: nil)
            let line = text.trimmingCharacters(in: .whitespacesAndNewlines)
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                let low = line.lowercased()
                let ok = code == 0 && !low.contains("not ") && !low.contains("unavailable")
                self.available = ok
                self.unavailableWhy = ok ? "" : "Apple Intelligence unavailable: " + (line.isEmpty ? "fm available failed" : line)
                if !ok { self.setStatus(self.unavailableWhy, tone: .danger) }
                else if self.process == nil && self.answer.isEmpty { self.setReady() }
                if ok { self.scheduleTokenCount() }
            }
        }
    }

    // run a short command to the end: (exit code, stdout + stderr)
    private static func capture(_ bin: String, _ args: [String], stdin: String?) -> (Int32, String) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: bin)
        p.arguments = args
        let out = Pipe(), inP = Pipe()
        p.standardOutput = out
        p.standardError = out
        if stdin != nil { p.standardInput = inP }
        do { try p.run() } catch { return (-1, error.localizedDescription) }
        if let s = stdin {
            let d = Data(s.utf8)
            DispatchQueue.global(qos: .utility).async {
                inP.fileHandleForWriting.write(d)
                try? inP.fileHandleForWriting.close()
            }
        }
        let text = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        p.waitUntilExit()
        return (p.terminationStatus, text)
    }

    private func setReady() {
        setStatus(rule.map { "Ready — Ctrl+Enter runs “\($0.name)”" } ?? "")
    }

    @objc private func runClicked() { process != nil ? cancelRun() : run() }

    private func run() {
        saveInput()
        // the file may have been edited since the last look
        if let r = rule { rules[selected] = AIRule.load(r.path); applyRule(loadInput: false) }
        guard let picked = rule else { newRule(); return }
        steps = AIRule.chain(picked)
        strip.set(steps.map(\.name))
        let r = steps[0]
        cmdLine.warning = r.warnings.joined(separator: ", ")
        if available == false { setStatus(unavailableWhy, tone: .danger); return }
        let text = input.string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            setStatus("Type something on the left first", tone: .warning)
            focusInput()
            return
        }
        cancelRun(quiet: true)
        // code the model can't touch; long text in parts that fit fm's window
        let g = r.protectCode ? CodeGuard(text) : nil
        guardCode = (g?.codes.isEmpty ?? true) ? nil : g
        stepNotes = []
        runGen += 1
        started = Date()
        answer = ""
        rendered = [:]
        renderGen += 1
        answerInput = text
        answerDiff = r.diff
        output.string = ""
        spinner.isHidden = false
        spinner.startAnimation(nil)
        runButton.title = "Stop  ⎋"
        body.needsLayout = true
        if mode.target != nil { showPreviewMessage("Waiting for the model…") }
        startStep(0, text: guardCode?.text ?? text)
    }

    // one rule of the run over `text` (the input, or the step before's answer)
    private func startStep(_ i: Int, text: String) {
        let r = steps[i]
        stepIndex = i
        strip.running(i)
        stepInput = r.prepare(text)
        let budget = TokenBudget.partBudget(instructions: r.instructions(guarded: guardCode != nil) + r.prompt)
        parts = r.chunk ? TokenBudget.parts(stepInput, budget: budget) : [stepInput]
        partIndex = 0
        doneParts = []
        runPart(r)
    }

    private func runPart(_ r: AIRule) {
        let gen = runGen
        let n = parts.count
        let stepNote = steps.count > 1 ? " \(r.name) (\(stepIndex + 1) of \(steps.count))" : ""
        setStatus("Asking the on-device model…" + stepNote + (n > 1 ? " part \(partIndex + 1) of \(n)" : ""))
        streamData = Data()
        let p = Process()
        p.executableURL = URL(fileURLWithPath: fmBin)
        p.arguments = r.arguments(guarded: guardCode != nil)
        let inPipe = Pipe(), outPipe = Pipe(), errPipe = Pipe()
        p.standardInput = inPipe
        p.standardOutput = outPipe
        p.standardError = errPipe
        do { try p.run() } catch {
            finish(gen: gen, code: -1, err: "couldn't start \(fmBin): \(error.localizedDescription)")
            return
        }
        process = p
        let data = Data(r.wrap(parts[partIndex]).utf8)
        DispatchQueue.global(qos: .userInitiated).async {
            inPipe.fileHandleForWriting.write(data)
            try? inPipe.fileHandleForWriting.close()
        }
        var errData = Data()
        let errDone = DispatchGroup()
        errDone.enter()
        DispatchQueue.global(qos: .utility).async {
            errData = errPipe.fileHandleForReading.readDataToEndOfFile()
            errDone.leave()
        }
        // stdout in chunks as fm streams; the finish is posted after the
        // last chunk (same thread, same order on main)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let h = outPipe.fileHandleForReading
            while true {
                let d = h.availableData
                if d.isEmpty { break }
                DispatchQueue.main.async { self?.chunk(gen: gen, d) }
            }
            p.waitUntilExit()
            errDone.wait()
            let err = String(decoding: errData, as: UTF8.self)
            let code = p.terminationStatus
            DispatchQueue.main.async { self?.finish(gen: gen, code: code, err: err) }
        }
    }

    // this step's answer so far (code still as tokens)
    private func stepSoFar() -> String {
        let cur = String(decoding: streamData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        let joined = (doneParts + (cur.isEmpty ? [] : [cur])).joined(separator: "\n\n")
        // a wrapper fence the model added (unless the text itself was one)
        return guardCode != nil || !answerInput.hasPrefix("```") ? AnswerCleanup.unwrapFence(joined) : joined
    }

    // code put back
    private func restored(_ s: String) -> (text: String, missing: Int) { guardCode?.restore(s) ?? (s, 0) }
    private func soFar() -> (text: String, missing: Int) { restored(stepSoFar()) }

    private func chunk(gen: Int, _ d: Data) {
        guard gen == runGen else { return }
        streamData.append(d)
        if mode.target == nil {
            output.textStorage?.setAttributedString(NSAttributedString(
                string: soFar().text, attributes: [.font: textFont, .foregroundColor: colors.text]))
            output.scrollToEndOfDocument(nil)
        }
    }

    private func finish(gen: Int, code: Int32, err: String) {
        guard gen == runGen else { return }
        process = nil
        guard steps.indices.contains(stepIndex) else { return }
        let step = steps[stepIndex]
        if code == 0, partIndex + 1 < parts.count {
            doneParts.append(String(decoding: streamData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
            partIndex += 1
            runPart(step)
            return
        }
        var code = code
        var stepText = stepSoFar()
        if code != 0, stepIndex > 0 {
            // a later step failed: the answer of the one before still stands
            let line = err.split(separator: "\n").map { String($0).trimmingCharacters(in: .whitespaces) }
                .last(where: { !$0.isEmpty }) ?? "exit \(code)"
            stepNotes.append("“\(step.name)” failed (\(stripANSI(line)))")
            strip.mark(stepIndex, .failed, note: stripANSI(line))
            stepText = stepInput
            code = 0
        } else if code == 0 {
            let a = step.accept(input: stepInput, answer: stepText)
            stepText = a.text
            if let n = a.note { stepNotes.append(n) }
            strip.mark(stepIndex, a.note == nil ? .done : .warn, note: a.note ?? "")
        } else {
            strip.mark(stepIndex, .failed)
        }
        streamData = Data()
        if code == 0, stepIndex + 1 < steps.count {
            startStep(stepIndex + 1, text: stepText)
            return
        }
        let result = restored(stepText)
        spinner.stopAnimation(nil)
        spinner.isHidden = true
        runButton.title = "Run  ⌃↩"
        body.needsLayout = true
        let secs = String(format: "%.1f s", Date().timeIntervalSince(started))
        answer = result.text
        renderAnswer()
        if code != 0 {
            let line = err.split(separator: "\n").map { String($0).trimmingCharacters(in: .whitespaces) }
                .last(where: { !$0.isEmpty }) ?? "exit \(code)"
            setStatus("fm failed: " + stripANSI(line), tone: .danger)
            return
        }
        let partsNote = parts.count > 1 ? " · \(parts.count) parts" : ""
        if !stepNotes.isEmpty {
            setStatus(stepNotes.joined(separator: " · ") + " · \(secs)", tone: .warning)
        } else if result.missing > 0 {
            setStatus("The model dropped \(result.missing) code block\(result.missing == 1 ? "" : "s") — check before sending"
                + partsNote, tone: .warning)
        } else if answerDiff {
            let n = CharDiff.changes(CharDiff.diff(answerInput, answer))
            setStatus((n == 0 ? "No changes ✓ · \(secs)" : "\(n) change\(n == 1 ? "" : "s") · \(secs)") + partsNote,
                      tone: n == 0 ? .success : nil)
        } else {
            setStatus("Done · \(secs)" + partsNote)
        }
    }

    private func cancelRun(quiet: Bool = false) {
        guard let p = process else { return }
        runGen += 1
        p.terminate()
        process = nil
        streamData = Data()
        spinner.stopAnimation(nil)
        spinner.isHidden = true
        runButton.title = "Run  ⌃↩"
        strip.set(strip.items.map(\.name))
        if !quiet { setStatus("Stopped") }
    }

    private func clearAnswer() {
        cancelRun(quiet: true)
        answer = ""
        answerInput = ""
        rendered = [:]
        output.string = ""
        renderAnswer()
        if available != false { setReady() }
    }

    // MARK: the right pane

    private func setMode(_ m: PaneMode) {
        mode = modes.contains(m) ? m : .markdown
        modeSeg.selected = modes.firstIndex(of: mode) ?? 0
        UserDefaults.standard.set(mode.rawValue, forKey: Self.modeKey)
        if let t = mode.target {
            target = t
            UserDefaults.standard.set(t.rawValue, forKey: Self.targetKey)
        }
        updateCopyTitle()
        renderAnswer()
    }

    private func updateCopyTitle() {
        copyButton.title = "⧉ Copy for " + target.title
        body.needsLayout = true
    }

    // Diff / Markdown in the text view; Outlook / Webex in the web view
    private func renderAnswer() {
        let preview = mode.target != nil
        outScroll.isHidden = preview
        web.isHidden = !preview
        if preview {
            showPreview()
            return
        }
        guard !answer.isEmpty else { output.string = ""; return }
        let base: [NSAttributedString.Key: Any] = [.font: textFont, .foregroundColor: colors.text]
        guard answerDiff && mode == .diff else {
            output.textStorage?.setAttributedString(NSAttributedString(string: answer, attributes: base))
            return
        }
        let ops = CharDiff.diff(answerInput, answer)
        let s = NSMutableAttributedString()
        let danger = colors.tone(.danger), success = colors.tone(.success)
        for o in ops {
            switch o.kind {
            case .same:
                s.append(NSAttributedString(string: o.text, attributes: base))
            case .del:
                let d = NSMutableAttributedString(string: o.text, attributes: [
                    .font: textFont, .foregroundColor: danger.withAlphaComponent(0.9),
                    .strikethroughStyle: NSUnderlineStyle.single.rawValue,
                    .strikethroughColor: danger,
                    .backgroundColor: danger.withAlphaComponent(0.16)])
                // a hair of room before the replacement ("their They're")
                if let last = o.text.utf16.indices.last {
                    let at = o.text.utf16.distance(from: o.text.utf16.startIndex, to: last)
                    d.addAttribute(.kern, value: 4, range: NSRange(location: at, length: 1))
                }
                s.append(d)
            case .ins:
                s.append(NSAttributedString(string: o.text, attributes: [
                    .font: textFont, .foregroundColor: colors.text,
                    .backgroundColor: success.withAlphaComponent(0.28)]))
            }
        }
        output.textStorage?.setAttributedString(s)
    }

    // the styled HTML for a target, made off the main thread (pandoc)
    private func withRendered(_ t: PasteTarget, _ done: @escaping (String?) -> Void) {
        if let h = rendered[t.rawValue] { done(h); return }
        let md = answer, gen = renderGen
        DispatchQueue.global(qos: .userInitiated).async {
            let h = RichText.html(md, for: t)
            DispatchQueue.main.async { [weak self] in
                guard let self, gen == self.renderGen, md == self.answer else { return }
                if let h { self.rendered[t.rawValue] = h }
                done(h)
            }
        }
    }

    private func showPreview() {
        guard let t = mode.target else { return }
        guard !answer.isEmpty else {
            showPreviewMessage(process != nil ? "Waiting for the model…"
                : "Run a rule — the answer shows here as it will look pasted into \(t.title).")
            return
        }
        guard RichText.available else {
            showPreviewMessage("pandoc not found at \(RichText.pandocBin) — brew install pandoc. "
                + "Copy puts the Markdown on the clipboard meanwhile.")
            return
        }
        withRendered(t) { [weak self] h in
            guard let self, self.mode.target == t else { return }
            guard let h else { self.showPreviewMessage("pandoc couldn't convert this text."); return }
            self.web.loadHTMLString(self.previewPage(h, t), baseURL: nil)
        }
    }

    private func previewPage(_ fragment: String, _ t: PasteTarget) -> String {
        let dim = css(colors.dim)
        let note = t == .webex ? "Webex has no tables: they paste as aligned text. Markdown also works if you paste as plain text."
            : "Pastes as rich text: tables, code and lists keep their formatting."
        let card = t == .outlook
            ? "background:#ffffff;border-radius:6px;padding:16px 18px;box-shadow:0 1px 3px rgba(0,0,0,.35)"
            : "background:#f4f5f7;border-radius:14px;padding:12px 14px;box-shadow:0 1px 3px rgba(0,0,0,.35)"
        let who = t == .webex ? "<div style=\"font:600 12px -apple-system;color:#555;margin-bottom:6px\">You · now</div>" : ""
        return """
        <html><head><meta charset="utf-8"><style>
        html,body{margin:0;background:transparent}
        .cap{font:600 10.5px -apple-system;letter-spacing:.06em;color:\(dim);margin:8px 12px 6px}
        .note{font:11px -apple-system;color:\(dim);margin:8px 12px 12px}
        .card{margin:0 10px;\(card)}
        a{color:#0f6cbd}
        </style></head><body>
        <div class="cap">\(t.title.uppercased()) · AS IT WILL PASTE</div>
        <div class="card">\(who)\(fragment)</div>
        <div class="note">\(note)</div>
        </body></html>
        """
    }

    private func showPreviewMessage(_ s: String) {
        let esc = s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
        web.loadHTMLString("""
        <html><body style="margin:0;background:transparent;font:13px -apple-system;color:\(css(colors.dim))">
        <div style="padding:14px 16px">\(esc)</div></body></html>
        """, baseURL: nil)
    }

    // links in the preview open in the browser
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        if action.navigationType == .linkActivated, let u = action.request.url {
            NSWorkspace.shared.open(u)
            decisionHandler(.cancel)
        } else {
            decisionHandler(.allow)
        }
    }

    private func stripANSI(_ s: String) -> String {
        s.replacingOccurrences(of: #"\u001B\[[0-9;]*[A-Za-z]"#, with: "", options: .regularExpression)
    }

    // MARK: tokens (fm's own count, a moment after typing stops)

    private func scheduleTokenCount() {
        tokenTimer?.invalidate()
        tokenTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: false) { [weak self] _ in
            self?.countTokens()
        }
    }

    private func countTokens() {
        guard let r = rule, available != false else { tokens.stringValue = ""; return }
        let text = input.string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { tokens.stringValue = ""; body.needsLayout = true; return }
        let g = r.protectCode ? CodeGuard(text) : nil
        let guarded = !(g?.codes.isEmpty ?? true)
        let send = r.wrap(r.prepare(guarded ? g!.text : text))
        let instr = r.instructions(guarded: guarded)
        tokenGen += 1
        let gen = tokenGen, bin = fmBin, ctx = TokenBudget.context
        let partsN = r.chunk ? TokenBudget.parts(send, budget: TokenBudget.partBudget(instructions: instr)).count : 1
        DispatchQueue.global(qos: .utility).async {
            let (code, out) = Self.capture(bin, ["count-tokens", "-q"] + (instr.isEmpty ? [] : ["-i", instr]),
                                           stdin: send)
            let n = Int(out.trimmingCharacters(in: .whitespacesAndNewlines))
            DispatchQueue.main.async { [weak self] in
                guard let self, gen == self.tokenGen else { return }
                guard code == 0, let n else { self.tokens.stringValue = ""; return }
                // the answer needs room too: about as long as the input
                let need = n + (n - TokenBudget.estimate(instr))
                var s = "\(n) / \(ctx) tokens"
                if guarded { s += " · \(g!.codes.count) code kept out" }
                if partsN > 1 { s += " · runs in \(partsN) parts" }
                self.tokens.stringValue = s
                self.tokens.textColor = need > ctx && partsN == 1 ? self.colors.tone(.warning) : self.colors.dim
                self.tokens.toolTip = "fm's on-device model reads instructions + your text and writes the answer "
                    + "in one \(ctx)-token window ([ai] context-tokens). Code is swapped for [[CODEn]] tokens "
                    + "(not counted, never changed); long text is split at blank lines."
                self.body.needsLayout = true
            }
        }
    }

    // MARK: copy

    @objc private func copyAnswer() {
        guard !answer.isEmpty else { showToast("Nothing to copy yet", symbol: "exclamationmark.circle"); return }
        let t = target, md = answer
        guard RichText.available else {
            copy(RichText.markdown(md, for: t), what: "Markdown (brew install pandoc for rich text)")
            return
        }
        withRendered(t) { [weak self] h in
            RichText.copy(markdown: md, fragment: h, for: t)
            self?.showToast(h == nil ? "Copied Markdown — pandoc failed" : "Copied for \(t.title) — paste away",
                            symbol: h == nil ? "exclamationmark.circle" : "checkmark.circle.fill")
        }
    }

    private func copyMarkdown() {
        guard !answer.isEmpty else { showToast("Nothing to copy yet", symbol: "exclamationmark.circle"); return }
        copy(answer, what: "Markdown")
    }

    private func copyCommand() {
        guard let r = rule else { return }
        copy(r.runnable(input: input.string), what: "the command")
    }

    private func copy(_ s: String, what: String?) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
        let msg = what.map { "Copied \($0)" } ?? aiSetting("copy-toast", "Copied to clipboard")
        showToast(msg, symbol: "checkmark.circle.fill")
    }

    private func css(_ c: NSColor) -> String {
        let x = c.usingColorSpace(.sRGB) ?? c
        return String(format: "rgba(%d,%d,%d,%.2f)", Int(x.redComponent * 255), Int(x.greenComponent * 255),
                      Int(x.blueComponent * 255), x.alphaComponent)
    }

    // Raycast-style pill, bottom-center (same look as PopupWindow.showToast)
    private func showToast(_ text: String, symbol: String?) {
        guard let root = window.contentView else { return }
        toast?.removeFromSuperview()
        let pill = NSView()
        pill.wantsLayer = true
        pill.layer?.backgroundColor = colors.crust.withAlphaComponent(0.94).cgColor
        pill.layer?.borderColor = colors.text.withAlphaComponent(0.10).cgColor
        pill.layer?.borderWidth = 1
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: 12.5, weight: .medium)
        label.textColor = colors.text
        label.sizeToFit()
        var iconW: CGFloat = 0
        let h: CGFloat = 32
        if let symbol, let img = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) {
            let iv = NSImageView(image: img)
            iv.symbolConfiguration = .init(pointSize: 12.5, weight: .medium)
            iv.contentTintColor = colors.tone(.success)
            let sz = iv.fittingSize
            iv.frame = NSRect(x: 16, y: (h - sz.height) / 2, width: sz.width, height: sz.height)
            pill.addSubview(iv)
            iconW = sz.width + 8
        }
        let w = min(ceil(label.frame.width + iconW + 32), root.bounds.width - 32)
        label.frame = NSRect(x: 16 + iconW, y: (h - label.frame.height) / 2,
                             width: w - 32 - iconW, height: label.frame.height)
        pill.addSubview(label)
        let y: CGFloat = root.isFlipped ? root.bounds.height - h - 56 : 56
        pill.frame = NSRect(x: (root.bounds.width - w) / 2, y: y, width: w, height: h)
        pill.layer?.cornerRadius = h / 2
        root.addSubview(pill, positioned: .above, relativeTo: nil)
        toast = pill
        pill.alphaValue = 0
        NSAnimationContext.runAnimationGroup { $0.duration = 0.15; pill.animator().alphaValue = 1 }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.4) { [weak pill, weak self] in
            guard let pill else { return }
            NSAnimationContext.runAnimationGroup({ $0.duration = 0.25; pill.animator().alphaValue = 0 }) {
                pill.removeFromSuperview()
                if self?.toast === pill { self?.toast = nil }
            }
        }
    }

    private func setStatus(_ s: String, tone: PopupTone? = nil) {
        status.stringValue = s
        status.textColor = tone.map { colors.tone($0) } ?? colors.dim
        status.toolTip = s
    }

    // MARK: keys

    private func focusInput() {
        window.makeFirstResponder(input)
    }

    override func handleKey(_ e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let cmd = mods.contains(.command), ctrl = mods.contains(.control)
        switch e.keyCode {
        case 53:                                                         // Esc
            // stop a run; idle: hide when AI's "Esc Hides Window" is on
            if process != nil { cancelRun() }
            else if onSlotHide != nil { controller?.slot.escapeAtTop(.ai) }
        case 36 where ctrl || cmd, 76 where ctrl || cmd: run()           // Ctrl/Cmd+Return
        case 37 where cmd: focusInput()                                  // Cmd+L
        case 44 where cmd: showShortcuts()                               // Cmd+/
        default: return webEditKey(e, in: web)                           // the preview
        }
        return true
    }

    // MARK: kitchen sink (the header icon)

    override func showIconMenu() {
        let menu = iconMenu(view: .ai)
        func add(_ t: String, _ f: @escaping () -> Void) { menu.addItem(menuItem(t, f)) }
        add("New Rule…") { [weak self] in self?.newRule() }
        if let r = rule {
            add("Edit “\(r.name)” in Notes") { [weak self] in self?.controller?.openNoteFile(r.path) }
        }
        add("Open Rules Folder") { [weak self] in
            guard let self else { return }
            NSWorkspace.shared.open(URL(fileURLWithPath: self.rulesDir))
        }
        add("Reload Rules") { [weak self] in self?.reloadRules() }
        menu.addItem(.separator())
        add("Copy for Outlook") { [weak self] in self?.setMode(.outlook); self?.copyAnswer() }
        add("Copy for Webex") { [weak self] in self?.setMode(.webex); self?.copyAnswer() }
        add("Copy as Markdown") { [weak self] in self?.copyMarkdown() }
        add("Copy Command") { [weak self] in self?.copyCommand() }
        add("Keyboard Shortcuts") { [weak self] in self?.showShortcuts() }
        popUpIconMenu(menu)
    }

    // commands.toml [shortcuts] "ai: …" lines, then "all: …"
    private func showShortcuts() {
        func lines(_ v: String) -> [String] {
            shortcutEntries.filter { $0.view == v }.map { "\($0.keys) — \($0.what)" }
        }
        let own = lines("ai"), all = lines("all")
        let text = (own.isEmpty ? [] : own) + (all.isEmpty ? [] : ["", "Everywhere:"] + all)
        let a = NSAlert()
        a.messageText = "AI Shortcuts"
        a.informativeText = text.isEmpty ? "Add \"ai: keys\" = \"what\" lines to [shortcuts] in commands.toml."
            : text.joined(separator: "\n")
        a.addButton(withTitle: "OK")
        a.beginSheetModal(for: window) { _ in }
    }

    // the file "+" writes for a new rule
    private static let ruleTemplate = """
    ---
    name: {name}
    # diff = show what changed vs your text · plain = just the answer
    output: plain
    greedy: true
    # guardrails: permissive-content-transformations
    # placeholder: hint shown in the empty input pane
    # prompt: a line put before your text, e.g. "Summarize this text:"
    # then: another-rule.md   (runs next, on this rule's answer)
    ---
    Describe what the model should do with the text it is given.
    Return ONLY the result — no commentary, no preamble.
    """
}
