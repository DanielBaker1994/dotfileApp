import AppKit
import WebKit

struct JiraBoardCard: Encodable {
    let key, title, type, priority, assignee, status: String
    let cat: Int
}

struct JiraBoardColumn: Encodable {
    let name: String
    let cards: [JiraBoardCard]
    let more: Int
}

final class JiraBoardBar: NSView, PopupThemeable {
    struct SprintChoice { let id: String; let title: String; let header: Bool }

    var onSprint: ((String) -> Void)?
    var onMode: ((String) -> Void)?
    var onPin: (() -> Void)?

    private let title = NSTextField(labelWithString: "")
    private let sprint = ThemedPopUpButton(frame: .zero, pullsDown: false)
    private let mode = ConfSegmented(["Columns", "Table"])
    private let summary = NSTextField(labelWithString: "")
    private let pin = ThemedPushButton(title: "Pin", target: nil, action: nil)
    private var colors = PopupThemeDefaults.colors
    private var choices: [SprintChoice] = []

    static let height: CGFloat = 40

    override init(frame: NSRect) {
        super.init(frame: frame)
        title.font = .systemFont(ofSize: 14, weight: .semibold)
        title.lineBreakMode = .byTruncatingTail
        summary.font = .systemFont(ofSize: 11.5)
        summary.lineBreakMode = .byTruncatingTail
        summary.setContentCompressionResistancePriority(.defaultLow - 10, for: .horizontal)
        sprint.controlSize = .small
        sprint.target = self
        sprint.action = #selector(sprintPicked)
        mode.onChange = { [weak self] i in self?.onMode?(i == 0 ? "columns" : "table") }
        mode.tips = ["The board's columns, a card per issue", "The issue table (sort, filter, group)"]
        pin.controlSize = .small
        pin.target = self
        pin.action = #selector(pinClicked)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.init(1), for: .horizontal)
        let stack = NSStackView(views: [title, sprint, mode, spacer, summary, pin])
        stack.orientation = .horizontal
        stack.spacing = 10
        stack.alignment = .centerY
        stack.edgeInsets = NSEdgeInsets(top: 0, left: 16, bottom: 0, right: 14)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }

    func applyColors(_ c: PopupColors) {
        colors = c
        title.textColor = c.text
        summary.textColor = c.dim
        sprint.applyColors(c)
        mode.applyColors(c)
        pin.colors = c
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        colors.hairline.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }

    func show(board: String, choices: [SprintChoice], picked: String, mode m: String, pinned: Bool, summary s: String) {
        title.stringValue = board
        self.choices = choices
        sprint.removeAllItems()
        sprint.isHidden = choices.isEmpty
        for c in choices {
            if c.header {
                let it = NSMenuItem(title: c.title, action: nil, keyEquivalent: "")
                it.isEnabled = false
                sprint.menu?.addItem(c.title.isEmpty ? .separator() : it)
            } else {
                sprint.addItem(withTitle: c.title)
                sprint.lastItem?.representedObject = c.id
            }
        }
        sprint.autoenablesItems = false
        if let it = sprint.itemArray.first(where: { ($0.representedObject as? String) == picked }) { sprint.select(it) }
        mode.selected = m == "table" ? 1 : 0
        pin.title = pinned ? "Pinned ✓" : "Pin to Sidebar"
        pin.toolTip = pinned ? "Unpin this view from the sidebar" : "Keep this exact view (board + sprint) as a row in the sidebar"
        summary.stringValue = s
        sprint.invalidateIntrinsicContentSize()
    }

    func setSummary(_ s: String) { summary.stringValue = s }

    @objc private func sprintPicked() {
        if let id = sprint.selectedItem?.representedObject as? String { onSprint?(id) }
    }
    @objc private func pinClicked() { onPin?() }
}

final class JiraBoardColumnsView: NSView, WKScriptMessageHandler, WKNavigationDelegate, PageZoomable {
    var pageZoom: CGFloat { get { web.pageZoom } set { web.pageZoom = newValue } }
    let web: WKWebView
    var onOpen: ((String) -> Void)?
    private var loaded = false
    private var pending: String?
    private var themeKey = ""

    override init(frame: NSRect) {
        let cfg = WKWebViewConfiguration()
        web = WKWebView(frame: frame, configuration: cfg)
        super.init(frame: frame)
        cfg.userContentController.add(WeakScriptHandler(self), name: "board")
        web.navigationDelegate = self
        web.setValue(false, forKey: "drawsBackground")
        web.autoresizingMask = [.width, .height]
        web.frame = bounds
        addSubview(web)
    }
    required init?(coder: NSCoder) { fatalError() }

    func show(_ cols: [JiraBoardColumn], colors: PopupColors) {
        let data = (try? JSONEncoder().encode(cols)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
        let key = [colors.background, colors.text, colors.dim, colors.accent, colors.highlight].map(ProseRender.css).joined()
        if key != themeKey {
            themeKey = key
            loaded = false
            pending = data
            // the page comes from pylib/jira_boards.py; colors go over as
            // hex/rgba strings (the theme algebra still lives in Swift)
            pythonHelper.call("jira.board_page", ["colors": Self.pageColors(colors)], timeout: 30) { [weak self] result in
                guard let self else { return }
                guard case .success(let box) = result, let d = box as? [String: Any],
                      let html = d["html"] as? String else { return }
                self.web.loadHTMLString(html, baseURL: nil)
            }
            return
        }
        guard loaded else { pending = data; return }
        web.evaluateJavaScript("render(\(data))")
    }

    private static func pageColors(_ c: PopupColors) -> [String: String] {
        ["bg": ProseRender.css(c.background), "text": ProseRender.css(c.text),
         "dim": ProseRender.css(c.dim), "accent": ProseRender.css(c.accent),
         "done": ProseRender.css(c.palette.success), "hot": ProseRender.css(c.palette.danger),
         "col": ProseRender.rgba(c.text, 0.045), "card": ProseRender.rgba(c.text, 0.07),
         "cardHover": ProseRender.rgba(c.text, 0.11), "line": ProseRender.rgba(c.text, 0.10)]
    }

    func userContentController(_ uc: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let s = message.body as? String else { return }
        if s == "ready" {
            loaded = true
            if let p = pending { pending = nil; web.evaluateJavaScript("render(\(p))") }
        } else if s.hasPrefix("open:") {
            onOpen?(String(s.dropFirst(5)))
        }
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        decisionHandler(action.navigationType == .other ? .allow : .cancel)
    }

}
