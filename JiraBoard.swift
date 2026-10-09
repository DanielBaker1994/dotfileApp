import AppKit
import WebKit

// The Jira board view (a board in the jira sidebar's BOARDS): the board's own
// columns, read from Jira, with a card per issue — like Jira's board. Above
// the search box: `JiraBoardBar` = board name · Sprint ▾ · Columns | Table ·
// Pin (the result as its own sidebar row). The cards are HTML over the list
// area (`PopupWindow.setListOverlay`); the search box, quick filters and the
// sprint narrow them like they narrow the table. A card click = the issue
// page (as Return on a row). ListSession (kitchen_sink.swift) owns the data.

struct JiraBoardCard: Encodable {
    let key, title, type, priority, assignee, status: String
    let cat: Int            // 0 to do / 1 in progress / 2 done
}

struct JiraBoardColumn: Encodable {
    let name: String
    let cards: [JiraBoardCard]
    let more: Int           // cards left out (a long Done column)
}

// MARK: - the bar

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

    // name, the sprint choices (empty = a board without sprints: no picker),
    // the picked one, columns / table, pinned?, the dim summary on the right
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

// MARK: - the columns

final class JiraBoardColumnsView: NSView, WKScriptMessageHandler, WKNavigationDelegate {
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

    // the columns on screen (a theme change reloads the page)
    func show(_ cols: [JiraBoardColumn], colors: PopupColors) {
        let data = (try? JSONEncoder().encode(cols)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
        let key = [colors.background, colors.text, colors.dim, colors.accent, colors.highlight].map(ProseRender.css).joined()
        if key != themeKey {
            themeKey = key
            loaded = false
            pending = data
            web.loadHTMLString(Self.page(colors), baseURL: nil)
            return
        }
        guard loaded else { pending = data; return }
        web.evaluateJavaScript("render(\(data))")
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

    // links never navigate the board away
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        decisionHandler(action.navigationType == .other ? .allow : .cancel)
    }

    private static func page(_ c: PopupColors) -> String {
        let css = ProseRender.css, rgba = ProseRender.rgba
        return """
        <!doctype html><html><head><meta charset="utf-8"><style>
        :root { --bg: \(css(c.background)); --col: \(rgba(c.text, 0.045)); --card: \(rgba(c.text, 0.07));
          --card-hover: \(rgba(c.text, 0.11)); --line: \(rgba(c.text, 0.10)); --text: \(css(c.text));
          --dim: \(css(c.dim)); --accent: \(css(c.accent)); --todo: \(css(c.dim));
          --prog: \(css(c.accent)); --done: \(css(c.palette.success)); --hot: \(css(c.palette.danger)); }
        * { box-sizing: border-box; }
        html, body { margin: 0; height: 100%; background: transparent; color: var(--text);
          font: 12.5px/1.4 -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif; }
        #board { display: flex; gap: 10px; padding: 12px 14px 14px; height: 100%; overflow-x: auto; }
        .col { flex: 1 0 210px; max-width: 360px; background: var(--col); border-radius: 10px; display: flex;
          flex-direction: column; min-height: 0; }
        .col h4 { margin: 0; padding: 10px 12px 8px; font-weight: 600; font-size: 10.5px; line-height: 1.2;
          letter-spacing: .08em; text-transform: uppercase; color: var(--dim); display: flex; gap: 8px; }
        .col h4 .n { margin-left: auto; font-variant-numeric: tabular-nums; }
        .cards { padding: 0 8px 8px; display: flex; flex-direction: column; gap: 7px; overflow-y: auto; min-height: 0; }
        .card { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px;
          display: grid; gap: 6px; cursor: default; }
        .card:hover { background: var(--card-hover); }
        .card .t { color: var(--text); overflow-wrap: anywhere; }
        .card .f { display: flex; align-items: center; gap: 8px; color: var(--dim); font-size: 11.5px; }
        .key { font: 11.5px ui-monospace, "SF Mono", Menlo, monospace; color: var(--accent); }
        .dot { width: 8px; height: 8px; border-radius: 2px; flex: none; border: 1.5px solid var(--todo); }
        .c1 .dot { border-color: var(--prog); background: linear-gradient(90deg, var(--prog) 50%, transparent 50%); }
        .c2 .dot { border-color: var(--done); background: var(--done); }
        .c2 .t { color: var(--dim); }
        .hot { color: var(--hot); }
        .av { margin-left: auto; width: 20px; height: 20px; border-radius: 50%; background: var(--line);
          color: var(--text); font-size: 9.5px; font-weight: 600; display: grid; place-items: center; flex: none; }
        .more, .empty { color: var(--dim); font-size: 11.5px; padding: 4px 4px 2px; }
        .none { color: var(--dim); padding: 40px; text-align: center; width: 100%; }
        </style></head><body><div id="board"></div><script>
        function esc(s) { return String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }
        function initials(n) { const p = n.trim().split(/\\s+/); return ((p[0]||'')[0]||'') + (p.length > 1 ? p[p.length-1][0] : ''); }
        function render(cols) {
          const b = document.getElementById('board');
          if (!cols.length || cols.every(c => !c.cards.length)) { b.innerHTML = '<div class="none">No issues here</div>'; return; }
          b.innerHTML = cols.map(c => `<section class="col"><h4>${esc(c.name)}<span class="n">${c.cards.length + c.more}</span></h4>
            <div class="cards">${c.cards.map(k => `<div class="card c${k.cat}" data-key="${esc(k.key)}" title="${esc(k.status)}">
              <div class="f"><span class="dot"></span><span class="key">${esc(k.key)}</span><span>${esc(k.type)}</span></div>
              <div class="t">${esc(k.title)}</div>
              <div class="f"><span class="${/highest|high|critical|blocker/i.test(k.priority) ? 'hot' : ''}">${esc(k.priority)}</span>
                ${k.assignee ? `<span class="av" title="${esc(k.assignee)}">${esc(initials(k.assignee).toUpperCase())}</span>` : ''}</div>
            </div>`).join('')}${c.more ? `<div class="more">+ ${c.more} more — Table shows them all</div>` : ''}
            ${!c.cards.length && !c.more ? '<div class="empty">Nothing here</div>' : ''}</div></section>`).join('');
        }
        document.addEventListener('click', e => {
          const c = e.target.closest('.card');
          if (c) window.webkit.messageHandlers.board.postMessage('open:' + c.dataset.key);
        });
        window.webkit.messageHandlers.board.postMessage('ready');
        </script></body></html>
        """
    }
}
