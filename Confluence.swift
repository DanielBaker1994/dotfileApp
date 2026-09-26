import AppKit
import WebKit

// MARK: - Confluence search (a shared-window view, Hyper+C)
//
// Search Confluence without leaving the app: a search strip (match mode All
// words / Phrase / Any word, Title only, spaces, type, modified, mine, sort),
// results on the left (title + excerpt with the matches marked, space ›
// parent path, author, age, ☆), the page rendered live on the right with
// every match highlighted and a "3 of 7" hits bar (Cmd+G / Shift+Cmd+G).
// Nothing is cached on disk: python (confluence/confluence_api.py) answers
// each search / page with JSON on stdout; the last 20 pages and their
// images stay in memory only.
//
//   Return (search box)   search (Favorites: open the highlighted page)
//   ↑↓ / Ctrl+N/P         move (preview follows)   Cmd+Return  open in browser
//   Cmd+C / ⇧Cmd+C        copy link / title + link
//   Cmd+D                 ☆ favorite       Cmd+1 / Cmd+2   Search / Favorites
//   Cmd+L / Cmd+F         search box       Cmd+G / ⇧Cmd+G  next / previous hit
//   Esc                   an open filter, else clear the query (never closes)
//
// Search and Favorites are separate: Search starts empty; Favorites is the
// pinned list for one-click opening (typing filters it, the Search button
// looks inside their text). Contributor = people seen editing the scope's
// spaces (confluence_api.py --users, cached a day). Rate limits: a long
// Retry-After pauses EVERY request (python's shared cooldown) with a
// countdown here, then the pending search / preview resumes by itself.
//
// Config: commands.conf [confluence] (enabled, width, height, colors);
// credentials + spaces + favorites: ~/.config/confluence/config.json (the
// Setup sheet). Fake site for development: bin/fake-confluence.sh start.

// [confluence] enabled - the view, its hotkey and its menu entries
func confluenceEnabled() -> Bool {
    ["true", "yes", "1", "on"].contains((configSectionValue("confluence", "enabled") ?? "").lowercased())
}

// a numeric [confluence] key (width / height / split)
func confluenceSetting(_ key: String, _ fallback: CGFloat) -> CGFloat {
    configSectionValue("confluence", key).flatMap { Double($0) }.map { CGFloat($0) } ?? fallback
}

// [confluence] colors over the theme (same keys as [jira]); none set = the
// Jira window's palette, so the Atlassian views read as one family
func confluenceColors() -> PopupColors {
    let keys = ["background-color", "text-color", "dim-color", "highlight-color", "accent-color", "palette"]
    guard keys.contains(where: { configSectionValue("confluence", $0) != nil }) else { return jiraWindowColors() }
    let v = { (k: String) in hexColor(configSectionValue("confluence", k)) }
    var c = PopupColors(background: v("background-color").map { ($0.usingColorSpace(.sRGB) ?? $0).withAlphaComponent(1) } ?? BAR,
                        border: BORDER, text: v("text-color") ?? TEXT, dim: v("dim-color") ?? DIM,
                        highlight: v("highlight-color") ?? GROUP_BG, accent: v("accent-color") ?? ACCENT,
                        palette: parsePalette(configSectionValue("confluence", "palette")) ?? THEME_PALETTE)
    if v("accent-color") != nil || THEME["border"] == nil { c.border = c.outline }
    return c
}

// MARK: - python bridge

enum ConfluenceAPI {
    static var dir: String { binDir + "/confluence" }

    // confluence_api.py ARGS (stdin = JSON) -> its one JSON object; a crash
    // or non-JSON output becomes {ok: false, error: <last stderr line>}
    static func run(_ args: [String], stdin: Any? = nil, done: @escaping ([String: Any]) -> Void) {
        var input: String?
        if let s = stdin, let d = try? JSONSerialization.data(withJSONObject: s) {
            input = String(decoding: d, as: UTF8.self)
        }
        JiraPoll.run("confluence_api.py", args, stdin: input ?? "", folder: dir) { code, out, err in
            let line = out.split(separator: "\n").last.map(String.init) ?? ""
            if let d = line.data(using: .utf8),
               let j = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] {
                done(j)
            } else {
                done(["ok": false, "error": JiraPoll.errorLine(err, fallback: "confluence_api.py exited \(code)")])
            }
        }
    }
}

// one search result / favorite
struct ConfluenceRow {
    var id = "", type = "page", title = "", excerpt = "", space = "", spaceName = "", path = ""
    var container = "", url = "", modified = "", modifiedText = "", author = ""
    var titleHits: [NSRange] = [], hits: [NSRange] = []
    var favorite = false, missing = false

    init(_ j: [String: Any]) {
        func s(_ k: String) -> String { (j[k] as? String) ?? (j[k].map { "\($0)" } ?? "") }
        func ranges(_ k: String) -> [NSRange] {
            ((j[k] as? [[Any]]) ?? []).compactMap { r in
                guard r.count == 2, let a = r[0] as? Int, let n = r[1] as? Int else { return nil }
                return NSRange(location: a, length: n)
            }
        }
        id = s("id"); type = s("type"); title = s("title"); excerpt = s("excerpt")
        space = s("space"); spaceName = s("spaceName"); path = s("path"); container = s("container")
        url = s("url"); modified = s("modified"); modifiedText = s("modifiedText"); author = s("author")
        titleHits = ranges("titleHits"); hits = ranges("hits")
        favorite = j["favorite"] as? Bool ?? false
        missing = j["missing"] as? Bool ?? false
    }

    // what --favorite add stores
    var json: [String: Any] {
        ["id": id, "title": title, "space": space, "spaceName": spaceName, "type": type, "url": url, "path": path]
    }

    var typeLabel: String {
        switch type {
        case "blogpost": return "Blog"
        case "attachment": return "Attachment"
        case "comment": return "Comment"
        default: return ""
        }
    }
}

// MARK: - small themed controls

// segmented choice: the chosen segment is an accent pill
final class ConfSegmented: NSView, PopupThemeable {
    var colors = JiraTheme.system { didSet { needsDisplay = true } }
    var items: [String] { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var tips: [String] = []
    var selected = 0 { didSet { needsDisplay = true } }
    var onChange: ((Int) -> Void)?
    private let font = NSFont.systemFont(ofSize: 12, weight: .medium)

    init(_ items: [String]) {
        self.items = items
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    func applyColors(_ c: PopupColors) { colors = c }

    private var widths: [CGFloat] {
        items.map { ceil(($0 as NSString).size(withAttributes: [.font: font]).width) + 20 }
    }
    override var intrinsicContentSize: NSSize {
        NSSize(width: widths.reduce(4, +), height: JiraTheme.height)
    }
    private func rect(_ i: Int) -> NSRect {
        let w = widths
        let x = 2 + w.prefix(i).reduce(0, +)
        return NSRect(x: x, y: 2, width: w[i], height: bounds.height - 4)
    }
    override func draw(_ dirty: NSRect) {
        JiraTheme.drawInput(bounds, colors, hover: false, focused: false)
        for (i, t) in items.enumerated() {
            let r = rect(i)
            let on = i == selected
            if on { ButtonStyle.draw(r, .on, colors, radius: JiraTheme.radius - 1) }
            let attrs: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: on ? ButtonStyle.text(.on, colors) : colors.dim]
            let sz = (t as NSString).size(withAttributes: attrs)
            (t as NSString).draw(at: NSPoint(x: r.midX - sz.width / 2, y: r.midY - sz.height / 2), withAttributes: attrs)
        }
    }
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let i = items.indices.first(where: { rect($0).contains(p) }), i != selected else { return }
        selected = i
        onChange?(i)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeAlways, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }
    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let i = items.indices.first { rect($0).contains(p) }
        toolTip = i.flatMap { $0 < tips.count ? tips[$0] : nil }
    }
}

// on / off button (Title only)
final class ConfToggle: NSView, PopupThemeable {
    var colors = JiraTheme.system { didSet { needsDisplay = true } }
    let title: String
    var isOn = false { didSet { needsDisplay = true } }
    var onChange: ((Bool) -> Void)?
    private let font = NSFont.systemFont(ofSize: 12, weight: .medium)

    init(_ title: String, tip: String) {
        self.title = title
        super.init(frame: .zero)
        toolTip = tip
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }
    func applyColors(_ c: PopupColors) { colors = c }
    override var intrinsicContentSize: NSSize {
        NSSize(width: ceil((title as NSString).size(withAttributes: [.font: font]).width) + 22, height: JiraTheme.height)
    }
    override func draw(_ dirty: NSRect) {
        let st: ButtonState = isOn ? .on : .idle
        ButtonStyle.draw(bounds, st, colors, radius: JiraTheme.radius)
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: ButtonStyle.text(isOn ? .on : .hover, colors)]
        let sz = (title as NSString).size(withAttributes: attrs)
        (title as NSString).draw(at: NSPoint(x: bounds.midX - sz.width / 2, y: bounds.midY - sz.height / 2),
                                 withAttributes: attrs)
    }
    override func mouseDown(with event: NSEvent) {
        isOn.toggle()
        onChange?(isOn)
    }
}

// a flipped container that lays its children out by hand
final class ConfPane: NSView {
    var onLayout: ((NSRect) -> Void)?
    var fill: NSColor? { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }
    override func layout() {
        super.layout()
        onLayout?(bounds)
    }
    override func draw(_ dirty: NSRect) {
        if let f = fill { f.setFill(); bounds.fill() }
    }
}

// the results table: a click in the star gutter toggles the favorite,
// right-click selects the row and shows its menu
final class ConfTableView: NSTableView {
    static let starGutter: CGFloat = 30
    var onStar: ((Int) -> Void)?
    var menuFor: ((Int) -> NSMenu?)?
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let r = row(at: p)
        if r >= 0, p.x < Self.starGutter {
            onStar?(r)
            return
        }
        super.mouseDown(with: event)
    }
    override func menu(for event: NSEvent) -> NSMenu? {
        let r = row(at: convert(event.locationInWindow, from: nil))
        guard r >= 0 else { return nil }
        selectRowIndexes(IndexSet(integer: r), byExtendingSelection: false)
        return menuFor?(r)
    }
}

// one result row, drawn: ☆ · title (matches marked) · meta · 2-line excerpt
final class ConfResultCell: NSView {
    var row: ConfluenceRow?
    var colors = JiraTheme.system
    override var isFlipped: Bool { true }

    static let height: CGFloat = 78

    private func marked(_ s: String, _ hits: [NSRange], font: NSFont, color: NSColor, bold: NSFont) -> NSAttributedString {
        let a = NSMutableAttributedString(string: s, attributes: [.font: font, .foregroundColor: color])
        let len = (s as NSString).length
        let mark = colors.accentOn.withAlphaComponent(colors.isLight ? 0.22 : 0.30)
        for r in hits where r.location >= 0 && NSMaxRange(r) <= len {
            a.addAttributes([.font: bold, .backgroundColor: mark, .foregroundColor: colors.text], range: r)
        }
        return a
    }

    override func draw(_ dirty: NSRect) {
        guard let row else { return }
        let x0 = ConfTableView.starGutter
        let w = bounds.width - x0 - 12
        // ☆ gutter
        let star = row.favorite ? "★" : "☆"
        let starColor = row.favorite ? colors.tone(.warning) : colors.dim.withAlphaComponent(0.55)
        (star as NSString).draw(at: NSPoint(x: 10, y: 7), withAttributes: [
            .font: NSFont.systemFont(ofSize: 14), .foregroundColor: starColor])
        // title (+ a type tag for non-pages)
        let tFont = NSFont.systemFont(ofSize: 13.5, weight: .semibold)
        let title = NSMutableAttributedString()
        if !row.typeLabel.isEmpty {
            title.append(NSAttributedString(string: row.typeLabel.uppercased() + "  ", attributes: [
                .font: NSFont.systemFont(ofSize: 9.5, weight: .bold), .foregroundColor: colors.tone(.accent2),
                .baselineOffset: 1]))
        }
        title.append(marked(row.title.isEmpty ? "(untitled)" : row.title, row.titleHits, font: tFont,
                            color: row.missing ? colors.dim : colors.text, bold: tFont))
        if row.missing {
            title.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue,
                               range: NSRange(location: 0, length: title.length))
        }
        let one: NSString.DrawingOptions = [.usesLineFragmentOrigin, .truncatesLastVisibleLine]
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        title.addAttribute(.paragraphStyle, value: para, range: NSRange(location: 0, length: title.length))
        title.draw(with: NSRect(x: x0, y: 6, width: w, height: 19), options: one)
        // meta: SPACE · parent path · author · age
        var meta = [row.spaceName.isEmpty ? row.space : row.spaceName]
        if !row.path.isEmpty { meta.append(row.path) } else if !row.container.isEmpty { meta.append("on " + row.container) }
        if !row.author.isEmpty { meta.append(row.author) }
        if !row.modifiedText.isEmpty { meta.append(row.modifiedText) }
        NSAttributedString(string: meta.filter { !$0.isEmpty }.joined(separator: "  ·  "), attributes: [
            .font: NSFont.systemFont(ofSize: 11), .foregroundColor: colors.dim, .paragraphStyle: para,
        ]).draw(with: NSRect(x: x0, y: 26, width: w, height: 16), options: one)
        // excerpt, two lines
        let wrap = NSMutableParagraphStyle()
        wrap.lineBreakMode = .byWordWrapping
        let ex = marked(row.excerpt, row.hits, font: .systemFont(ofSize: 12),
                        color: colors.text.withAlphaComponent(0.78), bold: .systemFont(ofSize: 12, weight: .semibold))
            .mutableCopy() as! NSMutableAttributedString
        ex.addAttribute(.paragraphStyle, value: wrap, range: NSRange(location: 0, length: ex.length))
        ex.draw(with: NSRect(x: x0, y: 43, width: w, height: 32), options: one)
    }
}

// wsconf://fetch/<base64url of the real URL>: page images + attachments,
// fetched with the Confluence credentials (a WKWebView can't add headers)
final class ConfluenceImageLoader: NSObject, WKURLSchemeHandler {
    static let scheme = "wsconf"
    var authHeader: String?
    private var cache: [String: (Data, String)] = [:]
    private var order: [String] = []
    private var tasks: [ObjectIdentifier: URLSessionDataTask] = [:]

    static func wrap(_ real: String) -> String {
        let b = Data(real.utf8).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        return "\(scheme)://fetch/\(b)"
    }

    static func unwrap(_ url: URL) -> URL? {
        var b = url.lastPathComponent.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while b.count % 4 != 0 { b += "=" }
        guard let d = Data(base64Encoded: b) else { return nil }
        return URL(string: String(decoding: d, as: UTF8.self))
    }

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url, let real = Self.unwrap(url) else {
            task.didFailWithError(URLError(.badURL))
            return
        }
        let key = real.absoluteString
        if let (data, mime) = cache[key] {
            task.didReceive(URLResponse(url: url, mimeType: mime, expectedContentLength: data.count, textEncodingName: nil))
            task.didReceive(data)
            task.didFinish()
            return
        }
        var req = URLRequest(url: real, timeoutInterval: 20)
        if let a = authHeader { req.setValue(a, forHTTPHeaderField: "Authorization") }
        let id = ObjectIdentifier(task)
        let dt = URLSession.shared.dataTask(with: req) { [weak self] data, resp, err in
            DispatchQueue.main.async {
                guard let self, self.tasks.removeValue(forKey: id) != nil else { return }   // stopped
                guard let data, err == nil, (resp as? HTTPURLResponse)?.statusCode ?? 0 < 400 else {
                    task.didFailWithError(err ?? URLError(.badServerResponse))
                    return
                }
                let mime = resp?.mimeType ?? "application/octet-stream"
                self.cache[key] = (data, mime)
                self.order.append(key)
                if self.order.count > 80 { self.cache.removeValue(forKey: self.order.removeFirst()) }
                task.didReceive(URLResponse(url: url, mimeType: mime, expectedContentLength: data.count,
                                            textEncodingName: nil))
                task.didReceive(data)
                task.didFinish()
            }
        }
        tasks[id] = dt
        dt.resume()
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
        tasks.removeValue(forKey: ObjectIdentifier(task))?.cancel()
    }
}

// MARK: - the window

final class ConfluenceWindow: NSObject, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate,
                              NSTextFieldDelegate, WKNavigationDelegate, WKScriptMessageHandler {
    private static var live: ConfluenceWindow?
    static var current: ConfluenceWindow? { live }

    private weak var controller: SwitcherController?
    let window: JiraConfigNSWindow
    private var chrome: PopupChrome?
    private var colors = confluenceColors()
    private var monitor: Any?
    var onSlotHide: (() -> Void)?
    private var slotNavClick: ((Int) -> Void)?

    // strip
    private let scopeSeg = ConfSegmented(["Search", "★ Favorites"])
    private let textBox = JiraInputBox(placeholder: "Search Confluence   \"exact phrase\"   prefix*",
                                       font: .systemFont(ofSize: 13))
    private let modeSeg = ConfSegmented(["All words", "Phrase", "Any word"])
    private let titleToggle = ConfToggle("Title only", tip: "Match page titles only")
    private let searchButton = ThemedPushButton(title: "Search", target: nil, action: nil)
    private let spaces = JiraMultiPicker(noun: "space", allTitle: "All spaces")
    private let typeChoice = JiraChoiceButton()
    private let modChoice = JiraChoiceButton()
    private let people = JiraMultiPicker(noun: "contributor", allTitle: "Any contributor")
    private let sortChoice = JiraChoiceButton()
    private let strip = ConfPane()

    // results
    private let table = ConfTableView()
    private let tableScroll = NSScrollView()
    private let status = NSTextField(labelWithString: "")
    private let moreButton = ThemedPushButton(title: "Load more", target: nil, action: nil)
    private let cqlButton = ThemedPushButton(title: "Copy CQL", target: nil, action: nil)
    private let curlButton = ThemedPushButton(title: "Copy curl", target: nil, action: nil)
    private let spinner = NSProgressIndicator()
    private let splitter = PaneSplitter()
    private var split: CGFloat = 0.42

    // preview
    private let preview = ConfPane()
    private let pTitle = NSTextField(labelWithString: "")
    private let pMeta = NSTextField(labelWithString: "")
    private let pStar = ThemedPushButton(title: "☆", target: nil, action: nil)
    private let pOpen = ThemedPushButton(title: "Open in Browser", target: nil, action: nil)
    private let pCopy = ThemedPushButton(title: "Copy Link", target: nil, action: nil)
    private let hitsBar = ConfPane()
    private let hitPrev = ThemedPushButton(title: "‹", target: nil, action: nil)
    private let hitNext = ThemedPushButton(title: "›", target: nil, action: nil)
    private let hitLabel = NSTextField(labelWithString: "")
    private let hint = NSTextField(wrappingLabelWithString: "")
    private let setupButton = ThemedPushButton(title: "Set Up Confluence…", target: nil, action: nil)
    private var web: WKWebView!
    private let images = ConfluenceImageLoader()

    // state
    private enum Scope { case search, favorites }
    private var scope: Scope = .search
    private var rows: [ConfluenceRow] = []
    private var favorites: [ConfluenceRow] = []
    private var lastCQL = "", lastCurl = "", nextLink = ""
    private var total = 0
    private var terms: [[String: Any]] = []
    private var site = ""
    private var configured = false
    private var searching = false
    private var searchGen = 0
    private var previewGen = 0
    private var previewTimer: Timer?
    private var previewID = ""
    private var pageCache: [String: [String: Any]] = [:]
    private var pageOrder: [String] = []
    private var criteriaTimer: Timer?
    private var lastFavRefresh: Date?
    // rate limit: every request waits for this (python refuses meanwhile)
    private var cooldownUntil: Date?
    private var cooldownTimer: Timer?
    private var pendingSearch: (() -> Void)?
    private var pendingPreview = false
    private var coolingDown: Bool { (cooldownUntil?.timeIntervalSinceNow ?? 0) > 0 }
    private static let criteriaKey = "confluenceCriteria"
    private static let splitKey = "confluenceSplit"

    // MARK: open

    static func create(controller: SwitcherController, frame: NSRect?) {
        guard live == nil else { return }
        let w = ConfluenceWindow(controller: controller, frame: frame)
        live = w
        w.reloadConfig()
    }

    // [app] shared-window = false: an ordinary window
    func showStandalone() {
        NSApp.activate(ignoringOtherApps: true)
        if !window.isVisible { window.center() }
        window.makeKeyAndOrderFront(nil)
        focusSearch()
    }

    private init(controller: SwitcherController, frame: NSRect?) {
        self.controller = controller
        let f = frame ?? NSRect(x: 0, y: 0, width: confluenceSetting("width", 1400), height: confluenceSetting("height", 900))
        window = JiraConfigNSWindow(contentRect: f, styleMask: [.titled, .closable, .resizable, .miniaturizable,
                                                                 .fullSizeContentView],
                                    backing: .buffered, defer: false)
        super.init()
        window.title = "Confluence"
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 820, height: 480)
        window.level = settings.float ? .popUpMenu : .normal
        window.delegate = self
        split = CGFloat(UserDefaults.standard.double(forKey: Self.splitKey))
        if split < 0.2 || split > 0.8 { split = confluenceSetting("split", 0.42) }
        PopupThemeDefaults.colors = colors
        window.contentView = themedRoot(buildContent())
        if frame != nil { window.setFrame(f, display: false) }
        restoreCriteria()
        installKeys()
    }

    // same surface as the Jira Config window: blur + card tint + border,
    // the popup header strip on top (✕ · kitchen sink · view icons · title)
    private func themedRoot(_ content: NSView) -> NSView {
        var cfg = PopupConfig(name: "confluence")
        cfg.colors = colors
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = hexColor(configSectionValue("confluence", "header-color")) ?? jiraHeaderColor
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
        window.appearance = NSAppearance(named: colors.isLight ? .aqua : .darkAqua)
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
        tint.layer?.backgroundColor = colors.base.withAlphaComponent(max(cfg.tintAlpha, 0.94)).cgColor
        tint.layer?.borderColor = colors.border.cgColor
        tint.layer?.borderWidth = 1
        tint.layer?.cornerRadius = radius
        let ch = PopupChrome(config: cfg)
        ch.dragHeaderHeight = cfg.headerHeight
        ch.headerIcon = confluenceAppIcon
        ch.headerTitle = "Confluence"
        ch.copyPathLabel = ""
        ch.copyConfigLabel = ""
        chrome = ch
        for v in [fx, tint, ch, content] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        for v in [fx, tint] as [NSView] {
            NSLayoutConstraint.activate([
                v.leadingAnchor.constraint(equalTo: root.leadingAnchor),
                v.trailingAnchor.constraint(equalTo: root.trailingAnchor),
                v.topAnchor.constraint(equalTo: root.topAnchor),
                v.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            ])
        }
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
        func walk(_ v: NSView) {
            (v as? PopupThemeable)?.applyColors(colors)
            v.subviews.forEach(walk)
        }
        walk(content)
        window.headerBand = cfg.headerHeight
        window.onHeaderClick = { [weak self] p in
            guard let self, let ch = self.chrome else { return }
            if ch.closeButtonRect.insetBy(dx: -2, dy: -2).contains(p) {
                self.closeOrHide()
            } else if let hit = ch.extraButtonRects.first(where: { $0.value.contains(p) }) {
                self.slotNavClick?(hit.key)
            } else if ch.headerIcon != nil, ch.iconButtonRect.insetBy(dx: -4, dy: -4).contains(p) {
                self.showIconMenu()
            }
        }
        return root
    }

    // the shared window's header: view icons (this one lit)
    func setSlotNav(icons: [(image: NSImage, id: Int, tip: String)], icon: NSImage, on: Int,
                    click: @escaping (Int) -> Void) {
        chrome?.navIcons = icons
        chrome?.navOn = on
        chrome?.headerIcon = icon
        chrome?.needsDisplay = true
        slotNavClick = click
    }

    private func closeOrHide() {
        if let hide = onSlotHide { hide() } else { window.orderOut(nil) }
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

    private func row(_ views: [NSView]) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .horizontal
        s.spacing = 8
        s.alignment = .centerY
        return s
    }

    private func buildContent() -> NSView {
        let root = ConfPane()

        // --- strip
        scopeSeg.tips = ["Search Confluence (Cmd+1)", "Your starred pages (Cmd+2) — type to filter, Return searches inside them"]
        scopeSeg.onChange = { [weak self] i in self?.setScope(i == 0 ? .search : .favorites) }
        modeSeg.tips = ["Every word somewhere in the page (AND)", "The words together, in this order",
                        "At least one of the words (OR)"]
        modeSeg.onChange = { [weak self] _ in self?.criteriaChanged() }
        titleToggle.onChange = { [weak self] _ in self?.criteriaChanged() }
        people.onChange = { [weak self] in self?.criteriaChanged() }
        people.placeholder = "Any contributor"
        people.options = [JiraMultiPicker.Option(id: "me", title: "Me", detail: "pages you created or edited")]
        textBox.field.delegate = self
        hook(searchButton, #selector(searchClicked(_:)), tip: "Search (Return)")
        searchButton.role = .primary
        spaces.onChange = { [weak self] in self?.criteriaChanged() }
        spaces.placeholder = "All spaces"
        typeChoice.items = [("Pages + blogs", "page,blogpost"), ("Pages", "page"), ("Blog posts", "blogpost"),
                            ("Attachments", "attachment"), ("Comments", "comment"),
                            ("Everything", "page,blogpost,attachment,comment")]
        typeChoice.value = "page,blogpost"
        typeChoice.prefix = "Type: "
        typeChoice.onPick = { [weak self] _ in self?.criteriaChanged() }
        modChoice.items = [("any time", ""), ("past 7 days", "7d"), ("past 30 days", "30d"),
                           ("past 90 days", "90d"), ("past year", "1y")]
        modChoice.value = ""
        modChoice.prefix = "Modified: "
        modChoice.onPick = { [weak self] _ in self?.criteriaChanged() }
        sortChoice.items = [("relevance", "relevance"), ("recently modified", "recent")]
        sortChoice.value = "relevance"
        sortChoice.prefix = "Sort: "
        sortChoice.onPick = { [weak self] _ in self?.criteriaChanged() }
        // how to search, top-left; the box under it, full width; filters below
        let top = row([scopeSeg, modeSeg, titleToggle, searchButton, NSView()])
        let middle = row([textBox])
        textBox.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let bottom = row([spaces, people, typeChoice, modChoice, sortChoice, NSView()])
        spaces.widthAnchor.constraint(equalToConstant: 240).isActive = true
        people.widthAnchor.constraint(equalToConstant: 240).isActive = true
        for s in [top, middle, bottom] {
            s.translatesAutoresizingMaskIntoConstraints = false
            strip.addSubview(s)
            NSLayoutConstraint.activate([
                s.leadingAnchor.constraint(equalTo: strip.leadingAnchor, constant: 12),
                s.trailingAnchor.constraint(equalTo: strip.trailingAnchor, constant: -12),
            ])
        }
        NSLayoutConstraint.activate([
            top.topAnchor.constraint(equalTo: strip.topAnchor, constant: 10),
            middle.topAnchor.constraint(equalTo: top.bottomAnchor, constant: 8),
            bottom.topAnchor.constraint(equalTo: middle.bottomAnchor, constant: 8),
        ])
        strip.fill = colors.mantle.withAlphaComponent(0.55)
        root.addSubview(strip)

        // --- results
        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("row"))
        col.resizingMask = .autoresizingMask
        table.addTableColumn(col)
        table.headerView = nil
        table.rowHeight = ConfResultCell.height
        table.intercellSpacing = NSSize(width: 0, height: 2)
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(openSelected)
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.focusRingType = .none
        table.allowsEmptySelection = true
        JC.table(table)
        table.onStar = { [weak self] r in self?.toggleFavorite(row: r) }
        table.menuFor = { [weak self] r in self?.contextMenu(row: r) }
        tableScroll.documentView = table
        tableScroll.hasVerticalScroller = true
        tableScroll.autohidesScrollers = true
        tableScroll.drawsBackground = false
        root.addSubview(tableScroll)
        label(status)
        root.addSubview(status)
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        root.addSubview(spinner)
        hook(moreButton, #selector(loadMore), tip: "Fetch the next page of results")
        hook(cqlButton, #selector(copyCQL), tip: "Copy the CQL of this search")
        hook(curlButton, #selector(copyCurl), tip: "Copy this search as a runnable curl ($CONFLUENCE_TOKEN)")
        for b in [moreButton, cqlButton, curlButton] { root.addSubview(b) }
        splitter.onFractionChange = { [weak self] f in
            guard let self else { return }
            self.split = f
            UserDefaults.standard.set(Double(f), forKey: Self.splitKey)
            root.needsLayout = true
        }
        root.addSubview(splitter)

        // --- preview
        preview.fill = colors.mantle.withAlphaComponent(0.35)
        label(pTitle, size: 15, weight: .semibold, color: colors.text)
        label(pMeta)
        hook(pStar, #selector(starPreview), tip: "Favorite (Cmd+D)")
        hook(pOpen, #selector(openSelected), tip: "Open the page in your browser (Return)")
        hook(pCopy, #selector(copyLink), tip: "Copy the page link (Cmd+C)")
        hook(hitPrev, #selector(prevHit), tip: "Previous match (Shift+Cmd+G)")
        hook(hitNext, #selector(nextHit), tip: "Next match (Cmd+G)")
        label(hitLabel, size: 12, color: colors.text.withAlphaComponent(0.85))
        hitsBar.fill = colors.mantle.withAlphaComponent(0.7)
        for v in [hitPrev, hitNext, hitLabel] as [NSView] { hitsBar.addSubview(v) }
        let wc = WKWebViewConfiguration()
        wc.setURLSchemeHandler(images, forURLScheme: ConfluenceImageLoader.scheme)
        wc.userContentController.add(WeakScriptHandler(self), name: "ws")
        web = WKWebView(frame: .zero, configuration: wc)
        web.navigationDelegate = self
        web.setValue(false, forKey: "drawsBackground")
        hint.font = .systemFont(ofSize: 13)
        hint.textColor = colors.dim
        hint.alignment = .center
        hook(setupButton, #selector(setupClicked), tip: "Site, email, token and the spaces to search")
        setupButton.role = .primary
        setupButton.controlSize = .regular
        for v in [pTitle, pMeta, pStar, pOpen, pCopy, hitsBar, web, hint, setupButton] as [NSView] { preview.addSubview(v) }
        root.addSubview(preview)

        // --- layout
        root.onLayout = { [weak self] b in self?.layoutAll(b) }
        showPreviewHint("")
        return root
    }

    private func layoutAll(_ b: NSRect) {
        let stripH: CGFloat = 10 + JiraTheme.height * 3 + 8 * 2 + 10
        strip.frame = NSRect(x: 0, y: 0, width: b.width, height: stripH)
        let bodyY = stripH
        let bodyH = b.height - stripH
        let leftW = (b.width * split).rounded()
        let footH: CGFloat = 32
        tableScroll.frame = NSRect(x: 0, y: bodyY + 4, width: leftW, height: bodyH - footH - 4)
        table.tableColumns.first?.width = leftW - 4
        // footer: spinner · status … Load more · Copy CQL · Copy curl
        let fy = b.height - footH + 5
        spinner.frame = NSRect(x: 12, y: fy + 3, width: 16, height: 16)
        var x = leftW - 10
        for btn in [curlButton, cqlButton, moreButton] {
            let w = btn.intrinsicContentSize.width
            x -= w
            btn.frame = NSRect(x: x, y: fy, width: w, height: 22)
            x -= 6
        }
        status.frame = NSRect(x: 32, y: fy + 3, width: max(40, x - 36), height: 16)
        splitter.frame = NSRect(x: leftW, y: bodyY, width: 6, height: bodyH)
        let px = leftW + 6
        preview.frame = NSRect(x: px, y: bodyY, width: b.width - px, height: bodyH)
        let pw = preview.bounds.width
        // header: title / meta | ☆ Open Copy
        var bx = pw - 12
        for btn in [pCopy, pOpen, pStar] {
            let w = max(28, btn.intrinsicContentSize.width)
            bx -= w
            btn.frame = NSRect(x: bx, y: 12, width: w, height: 22)
            bx -= 6
        }
        pTitle.frame = NSRect(x: 16, y: 10, width: max(40, bx - 22), height: 20)
        pMeta.frame = NSRect(x: 16, y: 32, width: pw - 32, height: 16)
        hitsBar.frame = NSRect(x: 0, y: 56, width: pw, height: 28)
        hitPrev.frame = NSRect(x: 10, y: 3, width: 26, height: 22)
        hitNext.frame = NSRect(x: 38, y: 3, width: 26, height: 22)
        hitLabel.frame = NSRect(x: 72, y: 6, width: pw - 84, height: 16)
        web.frame = NSRect(x: 0, y: 84, width: pw, height: max(0, preview.bounds.height - 84))
        hint.frame = NSRect(x: 40, y: preview.bounds.height / 2 - 60, width: max(0, pw - 80), height: 60)
        let sw = setupButton.intrinsicContentSize.width + 24
        setupButton.frame = NSRect(x: (pw - sw) / 2, y: preview.bounds.height / 2 + 8, width: sw, height: 30)
    }

    // MARK: config / setup

    // --check: the site, scope and where config.json is; builds the image
    // loader's Authorization header from that file (never logged)
    func reloadConfig(then: (() -> Void)? = nil) {
        ConfluenceAPI.run(["--check"]) { [weak self] j in
            guard let self else { return }
            self.configured = j["ok"] as? Bool ?? false
            self.site = j["site"] as? String ?? ""
            let sp = (j["spaces"] as? [[String: Any]]) ?? []
            self.spaces.options = sp.compactMap { s in
                guard let k = s["key"] as? String else { return nil }
                return JiraMultiPicker.Option(id: k, title: k, detail: s["name"] as? String ?? "")
            }
            self.spaces.allTitle = sp.isEmpty ? "All spaces" : "All spaces in scope"
            self.spaces.placeholder = self.spaces.allTitle ?? "All spaces"
            if let path = j["configPath"] as? String { self.loadAuth(path) }
            if !self.configured {
                let probs = (j["problems"] as? [String] ?? []).joined(separator: "; ")
                self.setStatus("Not set up yet: \(probs)", tone: .warning)
            } else {
                self.loadPeople(refresh: false)
            }
            if self.rows.isEmpty {
                if self.scope == .favorites {
                    self.loadFavorites(show: true)
                } else if self.configured && self.hasCriteria {
                    self.runSearch()             // back with a restored query
                } else {
                    self.showSearchEmpty()
                }
            }
            then?()
        }
    }

    private func loadAuth(_ path: String) {
        guard let d = FileManager.default.contents(atPath: path),
              let j = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] else {
            images.authHeader = nil
            return
        }
        let token = j["token"] as? String ?? "", email = j["email"] as? String ?? ""
        var auth = (j["auth"] as? String ?? "").lowercased()
        if auth != "bearer" && auth != "basic" { auth = email.isEmpty ? "bearer" : "basic" }
        guard !token.isEmpty else { images.authHeader = nil; return }
        images.authHeader = auth == "basic"
            ? "Basic " + Data("\(email):\(token)".utf8).base64EncodedString()
            : "Bearer " + token
    }

    // Setup: site, email (Cloud), token, spaces in scope. Save = detect the
    // auth mode that answers /user/current, then store it all
    func showSetup() {
        guard window.attachedSheet == nil else { return }
        if !window.isVisible { controller?.showConfluence() }
        ConfluenceAPI.run(["--check"]) { [weak self] j in
            guard let self else { return }
            let site = NSTextField(string: j["site"] as? String ?? "")
            site.placeholderString = "https://yourcompany.atlassian.net/wiki  or  https://confluence.corp/confluence"
            let email = NSTextField(string: j["email"] as? String ?? "")
            email.placeholderString = "Cloud: your Atlassian account email (empty for a Data Center token)"
            let token = NSSecureTextField(string: "")
            token.placeholderString = (j["hasToken"] as? Bool ?? false) ? "saved — leave empty to keep it"
                : "API token (Cloud) or personal access token (Data Center)"
            let oldKeys = ((j["spaces"] as? [[String: Any]]) ?? []).compactMap { $0["key"] as? String }
            let spacesF = NSTextField(string: oldKeys.joined(separator: ", "))
            spacesF.placeholderString = "space keys, e.g. ENG, OPS (empty = the whole site)"
            jiraFormSheet(on: self.window, title: "Confluence Setup",
                          info: "Confluence has its own site and token (not Jira's). Spaces work like Jira "
                              + "projects: searches stay inside them. Saved to \(j["configPath"] as? String ?? "config.json").",
                          rows: [("Site", site), ("Email", email), ("Token", token), ("Spaces", spacesF)],
                          ok: "Test & Save", first: site) { ok in
                guard ok else { return }
                let keys = spacesF.stringValue.uppercased()
                    .split(whereSeparator: { $0 == "," || $0 == " " }).map(String.init).filter { !$0.isEmpty }
                self.saveSetup(site: site.stringValue, email: email.stringValue, token: token.stringValue,
                               keys: keys, oldKeys: oldKeys)
            }
        }
    }

    private func saveSetup(site: String, email: String, token: String, keys: [String], oldKeys: [String]) {
        setStatus("Testing the connection…")
        spinner.startAnimation(nil)
        var given: [String: Any] = ["site": site, "email": email]
        if !token.isEmpty { given["token"] = token }
        ConfluenceAPI.run(["--detect-auth"], stdin: given) { [weak self] j in
            guard let self else { return }
            guard j["ok"] as? Bool == true else {
                self.spinner.stopAnimation(nil)
                let err = j["error"] as? String ?? "failed"
                let tried = ((j["tried"] as? [[String: Any]]) ?? [])
                    .map { "\($0["mode"] ?? "?"): \($0["error"] ?? $0["http"] ?? "")" }.joined(separator: "\n")
                self.setStatus("Setup: " + err, tone: .danger)
                self.alert("Couldn't sign in to Confluence", err + (tried.isEmpty ? "" : "\n\n" + tried),
                           buttons: ["Edit Setup", "Save Anyway", "Cancel"]) { i in
                    if i == 0 { self.showSetup() }
                    if i == 1 { self.store(given, keys: keys, oldKeys: oldKeys, user: "") }
                }
                return
            }
            var save = given
            save["auth"] = j["auth"] as? String ?? ""
            save["site"] = j["site"] as? String ?? site
            self.store(save, keys: keys, oldKeys: oldKeys, user: j["user"] as? String ?? "")
        }
    }

    private func store(_ save: [String: Any], keys: [String], oldKeys: [String], user: String) {
        ConfluenceAPI.run(["--save"], stdin: save) { [weak self] _ in
            guard let self else { return }
            let gone = oldKeys.filter { !keys.contains($0) }, new = keys.filter { !oldKeys.contains($0) }
            let finish = { (msg: String?) in
                self.spinner.stopAnimation(nil)
                self.reloadConfig {
                    if let msg { self.setStatus(msg, tone: .danger) } else {
                        self.setStatus("Connected" + (user.isEmpty ? "" : " as \(user)") + " · \(self.site)", tone: .success)
                    }
                }
            }
            let add = {
                guard !new.isEmpty else { finish(nil); return }
                ConfluenceAPI.run(["--add-space"] + new) { j in
                    finish(j["ok"] as? Bool == true ? nil : "Spaces: " + (j["error"] as? String ?? "failed"))
                }
            }
            if gone.isEmpty { add() } else { ConfluenceAPI.run(["--remove-space"] + gone) { _ in add() } }
        }
    }

    private func alert(_ title: String, _ text: String, buttons: [String], then: @escaping (Int) -> Void) {
        let a = NSAlert()
        a.messageText = title
        a.informativeText = text
        buttons.forEach { a.addButton(withTitle: $0) }
        a.beginSheetModal(for: window) { r in then(r.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue) }
    }

    // MARK: criteria

    private var query: String { textBox.field.stringValue.trimmingCharacters(in: .whitespaces) }

    private func criteria() -> [String: Any] {
        var c: [String: Any] = [
            "query": query, "mode": ["all", "phrase", "any"][modeSeg.selected], "titleOnly": titleToggle.isOn,
            "spaces": spaces.isAll ? [] : spaces.selected,
            "types": (typeChoice.value ?? "page,blogpost").split(separator: ",").map(String.init),
            "modified": modChoice.value ?? "", "contributors": people.isAll ? [] : people.selected,
            "sort": sortChoice.value ?? "relevance",
        ]
        if scope == .favorites { c["favorites"] = true }
        return c
    }

    private func saveCriteria() {
        var c = criteria()
        c.removeValue(forKey: "favorites")
        if let d = try? JSONSerialization.data(withJSONObject: c) {
            UserDefaults.standard.set(String(decoding: d, as: UTF8.self), forKey: Self.criteriaKey)
        }
    }

    private func restoreCriteria() {
        guard let s = UserDefaults.standard.string(forKey: Self.criteriaKey), let d = s.data(using: .utf8),
              let c = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] else { return }
        textBox.field.stringValue = c["query"] as? String ?? ""
        modeSeg.selected = ["all", "phrase", "any"].firstIndex(of: c["mode"] as? String ?? "all") ?? 0
        titleToggle.isOn = c["titleOnly"] as? Bool ?? false
        var who = c["contributors"] as? [String] ?? []
        if who.isEmpty, c["mine"] as? Bool == true { who = ["me"] }
        people.set(who, all: false)
        let sp = c["spaces"] as? [String] ?? []
        spaces.set(sp, all: sp.isEmpty)
        if let t = c["types"] as? [String], !t.isEmpty { typeChoice.value = t.joined(separator: ",") }
        modChoice.value = c["modified"] as? String ?? ""
        sortChoice.value = c["sort"] as? String ?? "relevance"
    }

    // anything to search for (a query, or a filter that narrows on its own)
    private var hasCriteria: Bool {
        !query.isEmpty || !(modChoice.value ?? "").isEmpty || (!people.isAll && !people.selected.isEmpty)
            || (!spaces.isAll && !spaces.selected.isEmpty)
    }

    // a filter changed: re-run what's on screen - debounced, so clicking
    // through a few filters sends ONE request (rate limits)
    private func criteriaChanged() {
        saveCriteria()
        criteriaTimer?.invalidate()
        criteriaTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: false) { [weak self] _ in
            guard let self else { return }
            if self.scope == .favorites {
                self.showingFavorites ? self.showFavorites() : self.runSearch()
            } else if self.hasCriteria {
                self.runSearch()
            }
        }
    }

    private func setScope(_ s: Scope) {
        scope = s
        scopeSeg.selected = s == .search ? 0 : 1
        searchButton.toolTip = s == .search ? "Search (Return)" : "Search inside your favorites' text"
        if s == .favorites {
            loadFavorites(show: true)
        } else if hasCriteria {
            runSearch()
        } else {
            showSearchEmpty()
        }
        focusSearch()
    }

    // Search with nothing typed: an empty list + how to start (never the
    // favorites - they live under ★ Favorites)
    private func showSearchEmpty() {
        showingFavorites = false
        lastCQL = ""
        nextLink = ""
        terms = []
        if configured {
            setRows([], empty: "Type to search \(site.isEmpty ? "Confluence" : site), then Return.\n"
                        + "★ Favorites (Cmd+2) keeps the pages you open often.")
            setStatus(coolingDown ? status.stringValue : "Ready")
        } else {
            rows = []
            table.reloadData()
            showPreviewHint("Connect a Confluence site to start searching.", setup: true)
        }
    }

    // MARK: search

    @objc private func searchClicked(_ sender: Any?) { runSearch() }

    private func runSearch(more: Bool = false) {
        guard configured else { showSetup(); return }
        var crit = criteria()
        if scope == .search && !hasCriteria && !more {
            showSearchEmpty()
            return
        }
        if coolingDown {
            pendingSearch = { [weak self] in self?.runSearch(more: more) }
            tickCooldown()
            return
        }
        if more {
            guard !nextLink.isEmpty else { return }
            crit["next"] = nextLink
        }
        saveCriteria()
        searchGen += 1
        let gen = searchGen
        searching = true
        spinner.startAnimation(nil)
        setStatus(more ? "Loading more…" : "Searching…")
        ConfluenceAPI.run(["--search"], stdin: crit) { [weak self] j in
            guard let self, gen == self.searchGen else { return }
            self.searching = false
            self.spinner.stopAnimation(nil)
            self.lastCQL = j["cql"] as? String ?? self.lastCQL
            self.lastCurl = j["curl"] as? String ?? self.lastCurl
            guard j["ok"] as? Bool == true else {
                if self.rateLimited(j, retry: { [weak self] in self?.runSearch(more: more) }) { return }
                if j["setup"] as? Bool == true { self.configured = false }
                self.setStatus(j["error"] as? String ?? "search failed", tone: .danger)
                if !more { self.setRows([]) }
                return
            }
            let new = ((j["results"] as? [[String: Any]]) ?? []).map(ConfluenceRow.init)
            self.showingFavorites = false
            self.terms = (j["terms"] as? [[String: Any]]) ?? []
            self.total = j["total"] as? Int ?? new.count
            self.nextLink = j["next"] as? String ?? ""
            if more {
                let sel = self.table.selectedRow
                self.rows += new
                self.table.reloadData()
                if sel >= 0 { self.select(sel) }
            } else {
                self.setRows(new)
            }
            let el = (j["elapsed"] as? Double).map { String(format: " · %.1fs", $0) } ?? ""
            let fb = j["fallback"] as? Bool == true ? " · no excerpts (older server)" : ""
            let within = self.scope == .favorites ? " in favorites" : ""
            self.setStatus(self.rows.isEmpty ? "No matches\(within) — try Any word, fewer filters, or prefix*"
                           : "\(self.rows.count) of \(self.total)\(within)\(el)\(fb)")
        }
    }

    @objc private func loadMore() { runSearch(more: true) }

    private func setRows(_ r: [ConfluenceRow], empty: String = "No results.") {
        rows = r
        table.reloadData()
        moreButton.isEnabled = !nextLink.isEmpty
        if rows.isEmpty {
            previewID = ""
            showPreviewHint(empty)
        } else {
            select(0)
        }
    }

    private func select(_ i: Int) {
        guard rows.indices.contains(i) else { return }
        table.selectRowIndexes(IndexSet(integer: i), byExtendingSelection: false)
        table.scrollRowToVisible(i)
    }

    // MARK: favorites

    // the saved list (refreshed in one request); show = put it on screen
    private func loadFavorites(show: Bool) {
        ConfluenceAPI.run(["--favorites", "--no-refresh"]) { [weak self] j in
            guard let self else { return }
            self.favorites = ((j["results"] as? [[String: Any]]) ?? []).map(ConfluenceRow.init)
            if show { self.showFavorites() }
            // the live refresh (one request) at most every 10 minutes
            guard self.configured, !self.favorites.isEmpty, !self.coolingDown,
                  (self.lastFavRefresh.map { Date().timeIntervalSince($0) > 600 } ?? true) else { return }
            self.lastFavRefresh = Date()
            ConfluenceAPI.run(["--favorites"]) { [weak self] j in
                guard let self, j["ok"] as? Bool == true, j["refreshed"] as? Bool == true else { return }
                self.favorites = ((j["results"] as? [[String: Any]]) ?? []).map(ConfluenceRow.init)
                if self.showingFavorites { self.showFavorites(keepSelection: true) }
            }
        }
    }

    private var showingFavorites = false

    // favorites filtered locally by the search box (title / space / path)
    private func showFavorites(keepSelection: Bool = false) {
        let words = query.lowercased().split(separator: " ").map(String.init)
        let sel = keepSelection && rows.indices.contains(table.selectedRow) ? rows[table.selectedRow].id : nil
        let list = favorites.filter { f in
            let hay = "\(f.title) \(f.space) \(f.spaceName) \(f.path)".lowercased()
            return words.allSatisfy { hay.contains($0) }
        }
        showingFavorites = true
        lastCQL = ""
        nextLink = ""
        terms = words.map { ["text": $0, "phrase": false, "prefix": true] }
        let empty = !favorites.isEmpty ? "No favorites match “\(query)”.\nThe Search button looks inside their text."
            : "No favorites yet.\nStar a search result (☆ or Cmd+D) to pin it here for one-click opening."
        setRows(list, empty: empty)
        if let sel, let i = rows.firstIndex(where: { $0.id == sel }) { select(i) }
        if !coolingDown {
            setStatus("★ \(list.count)" + (words.isEmpty ? "" : " of \(favorites.count)")
                      + " favorite\(favorites.count == 1 ? "" : "s") — Return opens, type to filter")
        }
    }

    private func toggleFavorite(row i: Int) {
        guard rows.indices.contains(i) else { return }
        let r = rows[i]
        let add = !r.favorite
        ConfluenceAPI.run(["--favorite", add ? "add" : "remove", r.id], stdin: add ? [r.json] : nil) { [weak self] j in
            guard let self else { return }
            guard j["ok"] as? Bool == true else {
                self.setStatus(j["error"] as? String ?? "favorite failed", tone: .danger)
                return
            }
            self.favorites = ((j["favorites"] as? [[String: Any]]) ?? []).map {
                var row = ConfluenceRow($0)
                row.favorite = true
                return row
            }
            if let k = self.rows.firstIndex(where: { $0.id == r.id }) {
                self.rows[k].favorite = add
                self.table.reloadData(forRowIndexes: IndexSet(integer: k), columnIndexes: IndexSet(integer: 0))
            }
            if self.showingFavorites && !add { self.showFavorites(keepSelection: true) }
            self.updatePreviewStar()
            self.setStatus(add ? "★ Added to favorites" : "Removed from favorites", tone: add ? .success : .dim)
        }
    }

    @objc private func starPreview() {
        if let i = rows.firstIndex(where: { $0.id == previewID }) { toggleFavorite(row: i) }
    }

    private func importSaved() {
        setStatus("Importing your saved pages…")
        spinner.startAnimation(nil)
        ConfluenceAPI.run(["--import-saved"]) { [weak self] j in
            guard let self else { return }
            self.spinner.stopAnimation(nil)
            guard j["ok"] as? Bool == true else {
                self.setStatus(j["error"] as? String ?? "import failed", tone: .danger)
                return
            }
            self.setStatus("Imported \(j["added"] ?? 0) of \(j["found"] ?? 0) saved pages", tone: .success)
            self.loadFavorites(show: self.showingFavorites)
        }
    }

    // MARK: table

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("confRow")
        let v = (tableView.makeView(withIdentifier: id, owner: nil) as? ConfResultCell) ?? {
            let c = ConfResultCell()
            c.identifier = id
            return c
        }()
        v.colors = colors
        v.row = rows[row]
        v.toolTip = rows[row].url
        v.needsDisplay = true
        return v
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let v = PopupTableRowView()
        v.colors = colors
        return v
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        previewTimer?.invalidate()
        let i = table.selectedRow
        guard rows.indices.contains(i) else { return }
        showHeader(rows[i])
        previewTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: false) { [weak self] _ in
            self?.loadPreview(i)
        }
        // near the end: fetch the next page
        if i >= rows.count - 2, !nextLink.isEmpty, !searching, !showingFavorites { loadMore() }
    }

    private var selectedRow: ConfluenceRow? {
        rows.indices.contains(table.selectedRow) ? rows[table.selectedRow] : nil
    }

    private func contextMenu(row i: Int) -> NSMenu? {
        guard rows.indices.contains(i) else { return nil }
        let r = rows[i]
        let m = NSMenu()
        func add(_ t: String, _ f: @escaping () -> Void) {
            let target = MenuActionTarget(action: f)
            menuActionTargets.append(target)
            let it = NSMenuItem(title: t, action: #selector(MenuActionTarget.run), keyEquivalent: "")
            it.target = target
            m.addItem(it)
        }
        add("Open in Browser") { [weak self] in self?.open(r) }
        add("Copy Link") { [weak self] in self?.copy(r.url, what: "link") }
        add("Copy Title + Link") { [weak self] in self?.copy("\(r.title)\n\(r.url)", what: "title + link") }
        add("Copy as Markdown Link") { [weak self] in self?.copy("[\(r.title)](\(r.url))", what: "markdown link") }
        m.addItem(.separator())
        add(r.favorite ? "Remove from Favorites" : "Add to Favorites") { [weak self] in self?.toggleFavorite(row: i) }
        if !r.space.isEmpty, !site.isEmpty {
            add("Open Space \(r.space) in Browser") { [weak self] in
                guard let self, let u = URL(string: self.site + "/spaces/" + r.space) else { return }
                NSWorkspace.shared.open(u)
            }
        }
        return m
    }

    @objc private func openSelected() { if let r = selectedRow { open(r) } }

    private func open(_ r: ConfluenceRow) {
        guard let u = URL(string: r.url), !r.url.isEmpty else { return }
        NSWorkspace.shared.open(u)
    }

    @objc private func copyLink() { if let r = selectedRow { copy(r.url, what: "link") } }

    private func copy(_ s: String, what: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(s, forType: .string)
        setStatus("Copied \(what)", tone: .success)
    }

    @objc private func copyCQL() { if !lastCQL.isEmpty { copy(lastCQL, what: "CQL") } }
    @objc private func copyCurl() { if !lastCurl.isEmpty { copy(lastCurl, what: "curl ($CONFLUENCE_TOKEN)") } }

    // MARK: preview

    @objc private func setupClicked() { showSetup() }

    private func showPreviewHint(_ text: String, setup: Bool = false) {
        hint.stringValue = text
        hint.isHidden = text.isEmpty
        setupButton.isHidden = !setup
        web.isHidden = true
        hitsBar.isHidden = true
        for v in [pTitle, pMeta, pStar, pOpen, pCopy] as [NSView] { v.isHidden = true }
    }

    private func showHeader(_ r: ConfluenceRow) {
        for v in [pTitle, pMeta, pStar, pOpen, pCopy] as [NSView] { v.isHidden = false }
        hint.isHidden = true
        setupButton.isHidden = true
        pTitle.stringValue = r.title
        pTitle.toolTip = r.title
        var meta = [r.spaceName.isEmpty ? r.space : r.spaceName]
        if !r.path.isEmpty { meta.append(r.path) }
        if !r.author.isEmpty { meta.append(r.author) }
        if !r.modifiedText.isEmpty { meta.append("updated " + r.modifiedText) }
        pMeta.stringValue = meta.filter { !$0.isEmpty }.joined(separator: "  ·  ")
        pMeta.toolTip = r.url
        previewID = r.id
        updatePreviewStar()
    }

    private func updatePreviewStar() {
        let fav = rows.first { $0.id == previewID }?.favorite ?? false
        pStar.title = fav ? "★" : "☆"
        pStar.toolTip = fav ? "Remove from favorites (Cmd+D)" : "Add to favorites (Cmd+D)"
    }

    private func loadPreview(_ i: Int) {
        guard rows.indices.contains(i) else { return }
        let r = rows[i]
        previewGen += 1
        let gen = previewGen
        if r.missing {
            web.isHidden = true
            hitsBar.isHidden = true
            hint.stringValue = "This page wasn't found — deleted, moved, or you lost access.\nRemove it with ★."
            hint.isHidden = false
            return
        }
        if let page = pageCache[r.id] { render(page, r); return }
        if coolingDown {
            pendingPreview = true
            web.isHidden = true
            hitsBar.isHidden = true
            hint.stringValue = "Paused for Confluence's rate limit — the preview loads when it lifts."
            hint.isHidden = false
            return
        }
        hitsBar.isHidden = false
        hitLabel.stringValue = "Loading page…"
        ConfluenceAPI.run(["--page", r.id]) { [weak self] j in
            guard let self, gen == self.previewGen else { return }
            guard j["ok"] as? Bool == true else {
                if self.rateLimited(j, retry: nil) {
                    self.pendingPreview = true
                    self.loadPreview(i)
                    return
                }
                self.web.isHidden = true
                self.hint.stringValue = "Couldn't load the page:\n" + (j["error"] as? String ?? "failed")
                self.hint.isHidden = false
                self.hitsBar.isHidden = true
                return
            }
            self.pageCache[r.id] = j
            self.pageOrder.append(r.id)
            if self.pageOrder.count > 20 { self.pageCache.removeValue(forKey: self.pageOrder.removeFirst()) }
            self.render(j, r)
        }
    }

    // the page HTML inside a themed template + the highlighter
    private func render(_ page: [String: Any], _ r: ConfluenceRow) {
        let base = (page["site"] as? String) ?? site
        var body = page["html"] as? String ?? ""
        let type = page["type"] as? String ?? r.type
        if type == "attachment" {
            let mt = page["mediaType"] as? String ?? ""
            let u = absolute(page["url"] as? String ?? r.url, base: base)
            body = mt.hasPrefix("image/") ? "<p><img src=\"\(u)\"></p>"
                : "<p class=dim>Attachment (\(mt.isEmpty ? "file" : mt)) — open it in the browser.</p>"
        }
        if body.isEmpty { body = "<p class=dim>(this page has no body)</p>" }
        let html = template(rewrite(body, base: base), title: page["title"] as? String ?? r.title)
        hint.isHidden = true
        web.isHidden = false
        hitsBar.isHidden = false
        hitLabel.stringValue = terms.isEmpty ? "" : "Finding matches…"
        web.loadHTMLString(html, baseURL: URL(string: base))
    }

    // a src / href as an absolute URL on the Confluence site
    private func absolute(_ s: String, base: String) -> String {
        if s.hasPrefix("http://") || s.hasPrefix("https://") || s.hasPrefix("data:") { return s }
        guard let b = URL(string: base), let scheme = b.scheme, let host = b.host else { return s }
        let origin = "\(scheme)://\(host)" + (b.port.map { ":\($0)" } ?? "")
        if s.hasPrefix("//") { return scheme + ":" + s }
        if s.hasPrefix("/") {
            // context-relative on a site with a context path (/wiki/...) or not
            let ctx = b.path
            if !ctx.isEmpty && ctx != "/" && !s.hasPrefix(ctx + "/") && s.hasPrefix("/download/") { return base + s }
            return origin + s
        }
        return base + "/" + s
    }

    // images + attachments on the site go through wsconf:// (credentials)
    private func rewrite(_ html: String, base: String) -> String {
        guard let host = URL(string: base)?.host,
              let rx = try? NSRegularExpression(pattern: "(<img\\b[^>]*?\\bsrc\\s*=\\s*)\"([^\"]+)\"",
                                                options: [.caseInsensitive]) else { return html }
        let ns = html as NSString
        var out = ""
        var last = 0
        for m in rx.matches(in: html, range: NSRange(location: 0, length: ns.length)) {
            out += ns.substring(with: NSRange(location: last, length: m.range.location - last))
            let pre = ns.substring(with: m.range(at: 1))
            let src = absolute(ns.substring(with: m.range(at: 2)).replacingOccurrences(of: "&amp;", with: "&"), base: base)
            let onSite = URL(string: src)?.host == host
            out += pre + "\"" + (onSite ? ConfluenceImageLoader.wrap(src) : src) + "\""
            last = NSMaxRange(m.range)
        }
        out += ns.substring(from: last)
        // srcset would bypass the rewrite: drop it
        return out.replacingOccurrences(of: "srcset=", with: "data-srcset=")
    }

    private func css(_ c: NSColor) -> String {
        let s = c.usingColorSpace(.sRGB) ?? c
        return String(format: "rgba(%d,%d,%d,%.3f)", Int(s.redComponent * 255), Int(s.greenComponent * 255),
                      Int(s.blueComponent * 255), s.alphaComponent)
    }

    private func template(_ body: String, title: String) -> String {
        let c = colors
        let termsJSON = (try? JSONSerialization.data(withJSONObject: terms)).map { String(decoding: $0, as: UTF8.self) } ?? "[]"
        return """
        <!doctype html><html><head><meta charset="utf-8">
        <style>
        :root { color-scheme: \(c.isLight ? "light" : "dark"); }
        html, body { background: transparent; }
        body { font: 14px/1.6 -apple-system, "SF Pro Text", sans-serif; color: \(css(c.text));
               margin: 0; padding: 14px 22px 80px; overflow-wrap: anywhere; }
        h1, h2, h3, h4 { line-height: 1.3; margin: 1.2em 0 .4em; }
        h1 { font-size: 1.5em; } h2 { font-size: 1.25em; } h3 { font-size: 1.1em; }
        a { color: \(css(c.accentOn)); }
        p, li { color: \(css(c.text.withAlphaComponent(0.92))); }
        .dim { color: \(css(c.dim)); }
        code { font: 12.5px ui-monospace, "SF Mono", monospace; background: \(css(c.mantle)); padding: 1px 4px; border-radius: 4px; }
        pre { font: 12.5px/1.45 ui-monospace, "SF Mono", monospace; background: \(css(c.mantle));
              padding: 10px 12px; border-radius: 6px; overflow: auto; border: 1px solid \(css(c.hairline)); }
        table { border-collapse: collapse; margin: .6em 0; }
        td, th { border: 1px solid \(css(c.hairline)); padding: 5px 9px; vertical-align: top; }
        th { background: \(css(c.mantle)); text-align: left; }
        img { max-width: 100%; height: auto; border-radius: 4px; }
        blockquote { border-left: 3px solid \(css(c.hairline)); margin: .6em 0; padding: 0 12px; color: \(css(c.dim)); }
        .confluence-information-macro, .panel, .aui-message { border-left: 3px solid \(css(c.accentOn));
              background: \(css(c.mantle)); padding: 2px 12px; margin: .8em 0; border-radius: 4px; }
        .confluence-information-macro-warning { border-color: \(css(c.tone(.warning))); }
        .confluence-information-macro-note { border-color: \(css(c.tone(.info))); }
        mark.wsh { background: \(css(c.tone(.warning).withAlphaComponent(0.35))); color: inherit; border-radius: 2px; padding: 0 1px; }
        mark.wsh.on { background: \(css(c.tone(.warning).withAlphaComponent(0.75))); color: #000;
                      box-shadow: 0 0 0 2px \(css(c.accentOn)); }
        .ws-title { font-size: 1.6em; font-weight: 650; margin: .2em 0 .6em; }
        </style></head><body><div class="ws-title">\(escapeHTML(title))</div>\(body)
        <script>
        (function () {
          const terms = \(termsJSON);
          const esc = s => s.replace(/[.*+?^${}()|[\\]\\\\]/g, '\\\\$&');
          const suf = '(?:s|es|ed|d|ing|ment|ments|er|ers|ly)?';
          const parts = terms.map(t => {
            const w = (t.text || '').trim().split(/\\s+/).filter(Boolean).map(x => esc(x) + (t.prefix ? '' : suf));
            if (!w.length) return null;
            return '\\\\b' + w.join('\\\\s+') + (t.prefix ? '\\\\w*' : '\\\\b');
          }).filter(Boolean);
          let marks = [], cur = -1;
          if (parts.length) {
            const rx = new RegExp(parts.join('|'), 'gi');
            const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
              acceptNode: n => (n.parentNode && /^(SCRIPT|STYLE|MARK)$/.test(n.parentNode.nodeName))
                ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT });
            const nodes = []; while (walker.nextNode()) nodes.push(walker.currentNode);
            for (const n of nodes) {
              const s = n.nodeValue; rx.lastIndex = 0; let m, last = 0, frag = null;
              while ((m = rx.exec(s)) && m[0].length) {
                frag = frag || document.createDocumentFragment();
                frag.appendChild(document.createTextNode(s.slice(last, m.index)));
                const mk = document.createElement('mark'); mk.className = 'wsh'; mk.textContent = m[0];
                frag.appendChild(mk); last = m.index + m[0].length;
                // the title is marked but not a stop: hits walk the body
                if (!(n.parentNode.closest && n.parentNode.closest('.ws-title'))) marks.push(mk);
              }
              if (frag) { frag.appendChild(document.createTextNode(s.slice(last))); n.parentNode.replaceChild(frag, n); }
            }
          }
          function snippet(mk) {
            let b = mk.parentElement; while (b && getComputedStyle(b).display === 'inline') b = b.parentElement;
            const t = (b ? b.innerText : mk.textContent).replace(/\\s+/g, ' ');
            const at = t.toLowerCase().indexOf(mk.textContent.toLowerCase());
            const a = Math.max(0, at - 70), z = Math.min(t.length, at + mk.textContent.length + 90);
            return (a > 0 ? '…' : '') + t.slice(a, z) + (z < t.length ? '…' : '');
          }
          function go(i) {
            if (!marks.length) { post(-1); return; }
            if (cur >= 0) marks[cur].classList.remove('on');
            cur = (i + marks.length) % marks.length;
            marks[cur].classList.add('on');
            marks[cur].scrollIntoView({ block: 'center', behavior: 'smooth' });
            post(cur);
          }
          function post(i) {
            window.webkit.messageHandlers.ws.postMessage({ i: i, n: marks.length, snippet: i >= 0 ? snippet(marks[i]) : '' });
          }
          window.wsNext = () => go(cur + 1);
          window.wsPrev = () => go(cur - 1);
          if (marks.length) go(0); else post(-1);
        })();
        </script></body></html>
        """
    }

    private func escapeHTML(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
    }

    func userContentController(_ uc: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let j = message.body as? [String: Any] else { return }
        let i = j["i"] as? Int ?? -1, n = j["n"] as? Int ?? 0
        let snip = j["snippet"] as? String ?? ""
        hitPrev.isEnabled = n > 1
        hitNext.isEnabled = n > 1
        if n == 0 {
            hitLabel.stringValue = terms.isEmpty ? "" : "No matches in the page text (matched in title, labels or attachments)"
            hitLabel.textColor = colors.dim
            return
        }
        let head = "\(i + 1) of \(n)"
        let s = NSMutableAttributedString(string: head + "   ", attributes: [
            .font: NSFont.systemFont(ofSize: 12, weight: .semibold), .foregroundColor: colors.text])
        s.append(NSAttributedString(string: "“" + snip + "”", attributes: [
            .font: NSFont.systemFont(ofSize: 12), .foregroundColor: colors.text.withAlphaComponent(0.75)]))
        hitLabel.attributedStringValue = s
        hitLabel.toolTip = snip
    }

    @objc private func nextHit() { web.evaluateJavaScript("window.wsNext && wsNext()") }
    @objc private func prevHit() { web.evaluateJavaScript("window.wsPrev && wsPrev()") }

    // links in the preview open in the browser
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        if action.navigationType == .linkActivated, let u = action.request.url {
            let real = u.scheme == ConfluenceImageLoader.scheme ? ConfluenceImageLoader.unwrap(u) : u
            if let real { NSWorkspace.shared.open(real) }
            decisionHandler(.cancel)
            return
        }
        decisionHandler(.allow)
    }

    // MARK: status + keys

    // a {rateLimited, retryIn} answer: pause everything, count down, resume
    private func rateLimited(_ j: [String: Any], retry: (() -> Void)?) -> Bool {
        guard j["rateLimited"] as? Bool == true else { return false }
        let secs = max(3, (j["retryIn"] as? Int) ?? 30)
        cooldownUntil = Date().addingTimeInterval(TimeInterval(secs))
        if let retry { pendingSearch = retry }
        cooldownTimer?.invalidate()
        cooldownTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.tickCooldown() }
        tickCooldown()
        return true
    }

    private func tickCooldown() {
        guard let until = cooldownUntil else { return }
        let left = Int(ceil(until.timeIntervalSinceNow))
        if left > 0 {
            setStatus("Confluence rate limit — requests paused, resuming in \(left)s", tone: .warning)
            spinner.stopAnimation(nil)
            return
        }
        cooldownTimer?.invalidate()
        cooldownTimer = nil
        cooldownUntil = nil
        setStatus("Resuming…")
        let search = pendingSearch
        pendingSearch = nil
        search?()
        if pendingPreview {
            pendingPreview = false
            if rows.indices.contains(table.selectedRow) { loadPreview(table.selectedRow) }
        }
    }

    // the Contributor picker: Me + people seen in the scope (cached a day)
    private func loadPeople(refresh: Bool) {
        ConfluenceAPI.run(["--users"] + (refresh ? ["--refresh"] : [])) { [weak self] j in
            guard let self else { return }
            if self.rateLimited(j, retry: nil) { return }
            let users = (j["users"] as? [[String: Any]]) ?? []
            self.people.options = [JiraMultiPicker.Option(id: "me", title: "Me", detail: "pages you created or edited")]
                + users.compactMap { u in
                    guard let id = u["id"] as? String, !id.isEmpty else { return nil }
                    return JiraMultiPicker.Option(id: id, title: u["name"] as? String ?? id,
                                                  detail: u["username"] as? String ?? "")
                }
            self.people.set(self.people.selected, all: self.people.isAll)
            if refresh {
                let partial = j["partial"] as? Bool == true ? " (partial — some pages were skipped)" : ""
                self.setStatus(j["ok"] as? Bool == true ? "\(users.count) people\(partial)"
                               : (j["error"] as? String ?? "people list failed"),
                               tone: j["ok"] as? Bool == true ? .success : .danger)
            }
        }
    }

    private func setStatus(_ s: String, tone: PopupTone = .dim) {
        status.stringValue = s
        status.toolTip = s
        status.textColor = tone == .dim ? colors.dim : colors.tone(tone)
        cqlButton.isEnabled = !lastCQL.isEmpty
        curlButton.isEnabled = !lastCurl.isEmpty
        moreButton.isEnabled = !nextLink.isEmpty
    }

    private func focusSearch() {
        window.makeFirstResponder(textBox.field)
        textBox.field.currentEditor()?.selectedRange = NSRange(location: (textBox.field.stringValue as NSString).length,
                                                               length: 0)
    }

    // search box: Return searches (favorites: typing filters, Return searches
    // inside them), ↑↓ / Ctrl+N/P move the results without leaving the box
    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        switch sel {
        case #selector(NSResponder.insertNewline(_:)):
            // Favorites are for opening: Return opens the highlighted one
            if scope == .favorites && showingFavorites {
                openSelected()
            } else {
                runSearch()
            }
            return true
        case #selector(NSResponder.moveDown(_:)):
            move(1)
            return true
        case #selector(NSResponder.moveUp(_:)):
            move(-1)
            return true
        default:
            return false
        }
    }

    // favorites: typing filters them (back from a search inside them too)
    func controlTextDidChange(_ obj: Notification) {
        if scope == .favorites { showFavorites() }
    }

    private func move(_ d: Int) {
        guard !rows.isEmpty else { return }
        let i = table.selectedRow < 0 ? 0 : min(rows.count - 1, max(0, table.selectedRow + d))
        select(i)
    }

    // Esc: popovers own it (the pickers); else clear the query; never closes
    private func escape() {
        if !query.isEmpty {
            textBox.field.stringValue = ""
            if scope == .favorites { showFavorites() } else if !hasCriteria { showSearchEmpty() }
            focusSearch()
        } else if window.firstResponder !== textBox.field.currentEditor() {
            focusSearch()
        }
    }

    private func installKeys() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self else { return e }
            // Esc closes an open filter (and only it), wherever the keys are
            if e.keyCode == 53, let open = [self.spaces, self.people].first(where: { $0.isOpen }) {
                open.closePopover()
                return nil
            }
            if let sheet = self.window.attachedSheet {
                return sheet.isKeyWindow && JiraEditKeys.route(e, in: sheet) ? nil : e
            }
            guard self.window.isKeyWindow else { return e }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command), shift = mods.contains(.shift), ctrl = mods.contains(.control)
            let fr = self.window.firstResponder
            let inWeb = (fr as? NSView)?.isDescendant(of: self.web) == true
            switch e.keyCode {
            case 53:                                                         // Esc
                self.escape()
                return nil
            case 13 where cmd: self.closeOrHide(); return nil                // Cmd+W
            case 37 where cmd, 3 where cmd && !shift: self.focusSearch(); return nil   // Cmd+L / Cmd+F
            case 5 where cmd: shift ? self.prevHit() : self.nextHit(); return nil      // Cmd+G
            case 2 where cmd:                                                // Cmd+D
                if self.rows.indices.contains(self.table.selectedRow) { self.toggleFavorite(row: self.table.selectedRow) }
                return nil
            case 18 where cmd, 83 where cmd: self.setScope(.search); return nil      // Cmd+1
            case 19 where cmd, 84 where cmd: self.setScope(.favorites); return nil   // Cmd+2
            case 15 where cmd: self.runSearch(); return nil                  // Cmd+R
            case 36 where cmd, 76 where cmd: self.openSelected(); return nil // Cmd+Return
            case 45 where ctrl: self.move(1); return nil                     // Ctrl+N
            case 35 where ctrl: self.move(-1); return nil                    // Ctrl+P
            default: break
            }
            if fr === self.table {
                if e.keyCode == 36 || e.keyCode == 76 { self.openSelected(); return nil }
                if cmd && e.keyCode == 8, let r = self.selectedRow {          // Cmd+C / Shift+Cmd+C
                    shift ? self.copy("\(r.title)\n\(r.url)", what: "title + link") : self.copy(r.url, what: "link")
                    return nil
                }
                // typing goes to the search box
                if !cmd && !ctrl, let ch = e.characters, ch.count == 1, ch.first?.isLetter == true
                    || ch.first?.isNumber == true {
                    self.focusSearch()
                    return e
                }
            }
            if inWeb && cmd {
                // the page: copy / select all like any document
                if e.keyCode == 8 { NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: nil); return nil }
                if e.keyCode == 0 { NSApp.sendAction(#selector(NSText.selectAll(_:)), to: nil, from: nil); return nil }
            }
            return JiraEditKeys.route(e, in: self.window) ? nil : e
        }
    }

    // MARK: kitchen sink (the header icon)

    private func showIconMenu() {
        guard let ch = chrome else { return }
        let menu = NSMenu()
        func add(_ t: String, _ f: @escaping () -> Void) {
            let target = MenuActionTarget(action: f)
            menuActionTargets.append(target)
            let it = NSMenuItem(title: t, action: #selector(MenuActionTarget.run), keyEquivalent: "")
            it.target = target
            menu.addItem(it)
        }
        add("Confluence Setup…") { [weak self] in self?.showSetup() }
        add("Import My Saved Pages as Favorites") { [weak self] in self?.importSaved() }
        add("Refresh Contributor List") { [weak self] in
            self?.setStatus("Refreshing people…")
            self?.loadPeople(refresh: true)
        }
        menu.addItem(.separator())
        add("Open config.json") {
            ConfluenceAPI.run(["--check"]) { j in
                if let p = j["configPath"] as? String, FileManager.default.fileExists(atPath: p) {
                    NSWorkspace.shared.open(URL(fileURLWithPath: p))
                }
            }
        }
        add("Open debug.log") {
            let p = NSHomeDirectory() + "/.cache/confluence/debug.log"
            if FileManager.default.fileExists(atPath: p) { NSWorkspace.shared.open(URL(fileURLWithPath: p)) }
        }
        menu.addItem(.separator())
        add("Keyboard Shortcuts") { [weak self] in
            self?.alert("Confluence Shortcuts", """
            Return — search · ↑↓ / Ctrl+N/P — move through results
            Return (in the list) / Cmd+Return / double-click — open in browser
            Cmd+C / Shift+Cmd+C — copy link / title + link
            Cmd+D or click ☆ — favorite · Cmd+1 / Cmd+2 — Search / Favorites
            Favorites: type to filter, Return opens, Search looks inside them
            Cmd+G / Shift+Cmd+G — next / previous match in the page
            Cmd+L / Cmd+F — search box · Cmd+R — search again
            Esc — close an open filter, else clear the query
            Cmd+W / ✕ — hide the window
            """, buttons: ["OK"]) { _ in }
        }
        ch.iconMenuOpen = true
        menu.popUp(positioning: nil, at: NSPoint(x: ch.iconButtonRect.minX, y: ch.iconButtonRect.maxY + 4), in: ch)
        ch.iconMenuOpen = false
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        closeOrHide()
        return false
    }
}

// WKUserContentController retains its handlers: break the cycle
private final class WeakScriptHandler: NSObject, WKScriptMessageHandler {
    weak var target: WKScriptMessageHandler?
    init(_ t: WKScriptMessageHandler) { target = t }
    func userContentController(_ uc: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(uc, didReceive: message)
    }
}

extension ConfluenceWindow: SlotMember {
    var slotWindow: NSWindow { window }
    var slotShown: Bool { window.isVisible }
    var slotBaseFrame: NSRect { window.frame }
    func slotPark(stopVoice: Bool) { window.orderOut(nil) }
    func slotShow(frame: NSRect?) {
        if let f = frame { window.setFrame(f, display: false) }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        if window.firstResponder === window || window.firstResponder == nil { focusSearch() }
    }
}
