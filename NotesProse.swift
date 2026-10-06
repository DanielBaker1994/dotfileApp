import AppKit
import WebKit

// Notes prose mode (Figma direction B, "Quiet Page"): the current note set
// as a READING page — Markdown rendered in a serif face (`[notes]
// prose-font`, default Literata → the system serif, New York) in a centred
// column of `prose-width` points, with a dim meta line (path · edited ·
// words) under the first heading. The switch at the editor's bottom right
// (Prose | Edit), ⌘⇧P or Esc goes back to the editor (nvim / native).
//
// Markdown → HTML: pandoc (`[ai] pandoc-bin`, the AI view's) when present,
// else `ProseRender.basic` — headings, lists + task boxes, fences, quotes,
// rules, paragraphs, inline code / bold / italic / links / images.

public struct ProseSource {
    public var markdown: String
    public var path: String
    public init(markdown: String, path: String) {
        self.markdown = markdown
        self.path = path
    }
}

enum ProseRender {
    static func esc(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;").replacingOccurrences(of: "\"", with: "&quot;")
    }

    static func css(_ c: NSColor) -> String {
        let s = ButtonStyle.opaque(c)
        return String(format: "#%02X%02X%02X", Int(s.redComponent * 255), Int(s.greenComponent * 255), Int(s.blueComponent * 255))
    }

    // inline spans of one line of text (already block-split)
    static func inline(_ raw: String) -> String {
        var s = esc(raw)
        func sub(_ pattern: String, _ tmpl: String) {
            if let re = try? NSRegularExpression(pattern: pattern) {
                s = re.stringByReplacingMatches(in: s, range: NSRange(s.startIndex..., in: s), withTemplate: tmpl)
            }
        }
        sub("`([^`]+)`", "<code>$1</code>")
        sub("!\\[([^\\]]*)\\]\\(([^)\\s]+)\\)", "<img alt=\"$1\" src=\"$2\">")
        sub("\\[([^\\]]+)\\]\\(([^)\\s]+)\\)", "<a href=\"$2\">$1</a>")
        sub("\\*\\*([^*]+)\\*\\*", "<strong>$1</strong>")
        sub("(?<![*\\w])\\*([^*\\s][^*]*)\\*(?!\\*)", "<em>$1</em>")
        sub("~~([^~]+)~~", "<del>$1</del>")
        return s
    }

    // the no-pandoc renderer: enough Markdown for notes
    static func basic(_ md: String) -> String {
        var out: [String] = []
        var para: [String] = []
        var list: String?          // "ul" / "ol" while inside a list
        var fence: [String]?
        func flushPara() {
            if !para.isEmpty { out.append("<p>" + para.map(inline).joined(separator: " ") + "</p>"); para = [] }
        }
        func closeList() { if let l = list { out.append("</\(l)>"); list = nil } }
        for line in md.components(separatedBy: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if var f = fence {
                if t.hasPrefix("```") { out.append("<pre><code>" + esc(f.joined(separator: "\n")) + "</code></pre>"); fence = nil }
                else { f.append(line); fence = f }
                continue
            }
            if t.hasPrefix("```") { flushPara(); closeList(); fence = []; continue }
            if t.isEmpty { flushPara(); closeList(); continue }
            if let m = t.range(of: "^#{1,6} ", options: .regularExpression) {
                flushPara(); closeList()
                let n = t[m].count - 1
                out.append("<h\(n)>" + inline(String(t[m.upperBound...])) + "</h\(n)>")
                continue
            }
            if t == "---" || t == "***" { flushPara(); closeList(); out.append("<hr>"); continue }
            if t.hasPrefix("> ") { flushPara(); closeList(); out.append("<blockquote>" + inline(String(t.dropFirst(2))) + "</blockquote>"); continue }
            let bullet = t.range(of: "^[-*+] ", options: .regularExpression)
            let number = t.range(of: "^[0-9]+[.)] ", options: .regularExpression)
            if let r = bullet ?? number {
                flushPara()
                let kind = bullet != nil ? "ul" : "ol"
                if list != kind { closeList(); out.append("<\(kind)>"); list = kind }
                var item = String(t[r.upperBound...])
                var box = ""
                if item.hasPrefix("[ ] ") { box = "<input type=\"checkbox\" disabled> "; item = String(item.dropFirst(4)) }
                else if item.lowercased().hasPrefix("[x] ") { box = "<input type=\"checkbox\" disabled checked> "; item = String(item.dropFirst(4)) }
                out.append("<li\(box.isEmpty ? "" : " class=\"task\"")>" + box + inline(item) + "</li>")
                continue
            }
            closeList()
            para.append(t)
        }
        if let f = fence { out.append("<pre><code>" + esc(f.joined(separator: "\n")) + "</code></pre>") }
        flushPara(); closeList()
        return out.joined(separator: "\n")
    }

    static func fragment(_ md: String) -> String {
        if let html = RichText.pandocHTML(md, highlight: true), !html.isEmpty { return html }
        return basic(md)
    }

    // GitHub alerts (> [!NOTE] …): pandoc gfm's `alerts` emits
    // <div class="note"><div class="title"><p>Note</p></div>…</div>
    static func alertCSS(_ c: PopupColors) -> String {
        let tones: [(String, PopupTone)] = [("note", .info), ("tip", .success), ("important", .accent2),
                                            ("warning", .warning), ("caution", .danger)]
        return tones.map { name, t in
            let hex = css(c.tone(t))
            return "div.\(name) { margin: 0 0 .9em; padding: .55em 1em .1em; border-left: 3px solid \(hex);"
                + " background: \(hex)14; border-radius: 6px; }"
                + " div.\(name) > .title p { margin: 0 0 .3em; color: \(hex); font-weight: 600;"
                + " font-family: -apple-system, system-ui; font-size: .85em; }"
        }.joined(separator: "\n")
    }

    // fenced code (pandoc `--syntax-highlighting` token spans) in palette tones
    static func codeCSS(_ c: PopupColors) -> String {
        let groups: [(String, String)] = [
            ("kw, cf", css(c.accentOn)), ("dt", css(c.tone(.accent2))),
            ("st, ch, ss, vs, sc", css(c.tone(.success))), ("dv, bn, fl, cn", css(c.tone(.warning))),
            ("fu, at, va, bu", css(c.tone(.info))), ("pp, im, ex, er, al", css(c.tone(.danger))),
            ("op, ot", css(c.text)),
        ]
        var out = groups.map { names, hex in
            names.split(separator: ",").map { "code span.\($0.trimmingCharacters(in: .whitespaces))" }
                .joined(separator: ", ") + " { color: \(hex); }"
        }
        out.append("code span.co, code span.do, code span.cv, code span.an, code span.in, code span.wa"
                   + " { color: \(css(c.dim)); font-style: italic; }")
        out.append("div.sourceCode { margin: 0 0 .9em; } div.sourceCode pre { margin: 0; }")
        out.append("pre.sourceCode a { border: 0; color: inherit; }")
        return out.joined(separator: "\n")
    }

    static func page(_ src: ProseSource, colors c: PopupColors, font: String, size: CGFloat, width: CGFloat) -> String {
        var body = fragment(src.markdown)
        // the meta line: under the first heading when the note starts with one
        let words = src.markdown.split { $0.isWhitespace || $0.isNewline }.count
        var edited = ""
        if let d = (try? FileManager.default.attributesOfItem(atPath: src.path))?[.modificationDate] as? Date {
            let f = DateFormatter()
            f.dateFormat = Calendar.current.isDateInToday(d) ? "HH:mm" : "MMM d, HH:mm"
            edited = " · edited " + f.string(from: d)
        }
        let meta = "<div class=\"meta\">\(esc((src.path as NSString).abbreviatingWithTildeInPath))\(edited) · \(words) words</div>"
        let trimmed = body.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("<h1"), let end = body.range(of: "</h1>") {
            body.insert(contentsOf: meta, at: end.upperBound)
        } else {
            body = meta + body
        }
        let base = URL(fileURLWithPath: (src.path as NSString).deletingLastPathComponent, isDirectory: true).absoluteString
        let accent = css(c.accentOn), text = css(c.text), dim = css(c.dim), bg = css(c.base)
        let well = css(c.mantle), rule = css(ButtonStyle.opaque(c.dim).blended(withFraction: 0.6, of: ButtonStyle.opaque(c.base)) ?? c.dim)
        return """
        <!doctype html><html><head><meta charset="utf-8"><base href="\(base)">
        <style>
        html { background: \(bg); }
        body { margin: 0; padding: 36px 32px 72px; color: \(text); font-family: \(font); font-size: \(Int(size))px;
               line-height: 1.66; -webkit-font-smoothing: antialiased; }
        main { max-width: \(Int(width))px; margin: 0 auto; }
        h1, h2, h3, h4 { font-family: \(font); font-weight: 600; line-height: 1.25; margin: 1.4em 0 .5em; }
        h1 { font-size: 1.6em; margin-top: 0; } h2 { font-size: 1.2em; } h3 { font-size: 1.05em; }
        .meta { font-family: -apple-system, system-ui; font-size: 11px; color: \(dim); margin: -.2em 0 1.6em; }
        p { margin: 0 0 .9em; }
        a { color: \(accent); text-decoration: none; border-bottom: 1px solid \(accent)55; }
        code { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: .84em; background: \(well);
               padding: .1em .35em; border-radius: 4px; }
        pre { background: \(well); padding: 12px 14px; border-radius: 8px; overflow-x: auto; line-height: 1.45; }
        pre code { background: none; padding: 0; color: \(css(c.tone(.accent2))); }
        blockquote { margin: 0 0 .9em; padding-left: 14px; border-left: 2px solid \(accent); color: \(dim); }
        \(alertCSS(c))
        \(codeCSS(c))
        hr { border: 0; border-top: 1px solid \(rule); margin: 1.6em 0; }
        ul, ol { padding-left: 1.4em; margin: 0 0 .9em; }
        li { margin: .2em 0; }
        li.task, li:has(> input[type=checkbox]) { list-style: none; margin-left: -1.4em; }
        input[type=checkbox] { appearance: none; width: 15px; height: 15px; border: 1.5px solid \(dim); border-radius: 4px;
               vertical-align: -2px; margin: 0 8px 0 0; position: relative; }
        input[type=checkbox]:checked { background: \(accent); border-color: \(accent); }
        input[type=checkbox]:checked::after { content: "✓"; position: absolute; left: 2px; top: -3px; font-size: 12px;
               color: \(css(c.onAccent)); font-family: system-ui; }
        li:has(> input:checked) { color: \(dim); text-decoration: line-through; }
        img { max-width: 100%; border-radius: 6px; }
        table { border-collapse: collapse; margin: 0 0 1em; font-size: .92em; }
        td, th { border: 1px solid \(rule); padding: 4px 10px; }
        ::selection { background: \(accent)44; }
        </style></head><body><main>\(body)</main></body></html>
        """
    }
}

// the reading page itself (a WKWebView painted in the card color)
final class ProseView: NSView, WKScriptMessageHandler {
    let web: WKWebView
    var onLinkClick: ((URL) -> Void)?
    var onOpenImage: ((String) -> Void) = { FilePopup.show(path: $0, over: nil) }
    private var lastPath = ""
    private var gen = 0

    override init(frame: NSRect) {
        let cfg = WKWebViewConfiguration()
        // double-click a picture -> its full-size popup (like the nvim view)
        cfg.userContentController.addUserScript(WKUserScript(source: """
            document.addEventListener('dblclick', function (e) {
              var t = e.target;
              if (t && t.tagName === 'IMG') window.webkit.messageHandlers.wsImage.postMessage(t.src);
            });
            """, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let w = ProseWebView(frame: frame, configuration: cfg)
        web = w
        super.init(frame: frame)
        w.onMenu = { [weak self] menu in self?.addMenuItems(menu) }
        cfg.userContentController.add(WeakScriptHandler(self), name: "wsImage")
        web.autoresizingMask = [.width, .height]
        web.frame = bounds
        web.setValue(false, forKey: "drawsBackground")
        addSubview(web)
        wantsLayer = true
    }
    required init?(coder: NSCoder) { fatalError() }

    // Mac-style zoom: Cmd+= / Cmd+- / Cmd+0, a trackpad pinch, a two-finger
    // double-tap (back to 100%). The page reflows (WKWebView.pageZoom), no
    // pandoc run; the last level is kept across launches.
    static let zoomKey = "proseZoom"
    var onZoom: ((CGFloat) -> Void)?
    var zoom: CGFloat {
        get { web.pageZoom }
        set {
            web.pageZoom = min(4, max(0.5, newValue))
            onZoom?(web.pageZoom)
        }
    }
    func restoreZoom() {
        let z = CGFloat(UserDefaults.standard.double(forKey: Self.zoomKey))
        if z > 0 { web.pageZoom = min(4, max(0.5, z)) }
    }
    func zoom(by factor: CGFloat) { zoom = zoom * factor }
    func resetZoom() { zoom = 1 }
    func saveZoom() { UserDefaults.standard.set(Double(web.pageZoom), forKey: Self.zoomKey) }
    override func magnify(with e: NSEvent) {
        zoom(by: 1 + e.magnification)
        if e.phase == .ended || e.phase == .cancelled { saveZoom() }
    }
    override func smartMagnify(with e: NSEvent) { resetZoom(); saveZoom() }

    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        guard let s = m.body as? String, let u = URL(string: s), u.isFileURL else { return }
        onOpenImage(u.path)
    }

    // right-click: the page's own items + Export PDF / Copy Note Path
    private func addMenuItems(_ menu: NSMenu) {
        guard !lastPath.isEmpty else { return }
        menu.addItem(.separator())
        let pdf = menuItem("Export PDF") { [weak self] in self?.exportPDF() }
        pdf.keyEquivalent = "p"
        pdf.keyEquivalentModifierMask = .command
        menu.addItem(pdf)
        let path = lastPath
        menu.addItem(menuItem("Copy Note Path") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(path, forType: .string)
        })
    }

    // ⌘P / right-click: the shown note → PDF (ProsePDF), its path on the
    // clipboard + the toast. The pandoc + weasyprint run is off main.
    private static var exporting = false
    func exportPDF() {
        let note = lastPath
        guard !note.isEmpty, !Self.exporting else { return }
        Self.exporting = true
        let screen = window?.screen
        var c = ProsePDF.Config()
        c.pandoc = RichText.pandocBin
        func notes(_ k: String) -> String? {
            configSectionValue("notes", k).map { $0.trimmingCharacters(in: .whitespaces) }.flatMap { $0.isEmpty ? nil : $0 }
        }
        if let v = notes("pdf-engine-bin") { c.engine = v }
        if let v = notes("pdf-css") { c.css = v }
        if let v = notes("pdf-path") { c.outDir = v }
        if let v = notes("pdf-highlight") { c.highlight = v }
        DispatchQueue.global(qos: .userInitiated).async {
            let r = ProsePDF.export(note: note, c)
            DispatchQueue.main.async {
                Self.exporting = false
                switch r {
                case .success(let out):
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(out, forType: .string)
                    let tilde = (out as NSString).abbreviatingWithTildeInPath
                    let fmt = settings.copyToast.isEmpty ? "Copied {} to clipboard" : settings.copyToast
                    ScreenToast.show(fmt.replacingOccurrences(of: "{}", with: tilde), on: screen, symbol: "doc.richtext")
                case .failure(let e):
                    ScreenToast.show(e.description, on: screen, symbol: "exclamationmark.triangle.fill")
                }
            }
        }
    }

    // render off the main thread (pandoc), keep the scroll spot on a reload
    // of the same note
    func show(_ src: ProseSource, colors: PopupColors, font: String, size: CGFloat, width: CGFloat) {
        layer?.backgroundColor = colors.base.cgColor
        gen += 1
        let g = gen
        let same = src.path == lastPath
        lastPath = src.path
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let html = ProseRender.page(src, colors: colors, font: font, size: size, width: width)
            DispatchQueue.main.async {
                guard let self, g == self.gen else { return }
                let dir = NSHomeDirectory() + "/.cache/workspace-switcher/prose"
                try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
                let file = URL(fileURLWithPath: dir + "/page.html")
                guard (try? Data(html.utf8).write(to: file)) != nil else { return }
                if same {
                    self.web.evaluateJavaScript("window.scrollY") { y, _ in
                        let y = (y as? Double) ?? 0
                        self.web.loadFileURL(file, allowingReadAccessTo: URL(fileURLWithPath: "/"))
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                            self.web.evaluateJavaScript("window.scrollTo(0, \(y))")
                        }
                    }
                } else {
                    self.web.loadFileURL(file, allowingReadAccessTo: URL(fileURLWithPath: "/"))
                }
            }
        }
    }
}

// the page's web view: lets ProseView add to its right-click menu
final class ProseWebView: WKWebView {
    var onMenu: ((NSMenu) -> Void)?
    override func willOpenMenu(_ menu: NSMenu, with event: NSEvent) {
        super.willOpenMenu(menu, with: event)
        onMenu?(menu)
    }
}

// Prose | Edit — a small capsule switch at the editor's bottom right (the
// view switcher's look: a track with the current mode on a raised chip)
final class ProseModeSwitch: NSView {
    var colors: PopupColors { didSet { needsDisplay = true } }
    var prose = false { didSet { needsDisplay = true } }
    var editLabel = "Edit"
    var onChange: ((Bool) -> Void)?
    var onPopOut: (() -> Void)?          // the ⤢ chip: the page in its own floating window
    private var hover: Int?
    private var tracking: NSTrackingArea?
    private let labels: [(String, String)]
    override var isFlipped: Bool { true }

    init(colors: PopupColors, editLabel: String) {
        self.colors = colors
        self.editLabel = editLabel
        labels = [("text.alignleft", "Prose"), ("chevron.left.forwardslash.chevron.right", editLabel)]
        super.init(frame: NSRect(x: 0, y: 0, width: 150, height: 28))
        frame.size.width = segW.reduce(6, +) + 34
        toolTip = "Reading view ⌘⇧P — Esc goes back to the editor; ⤢ opens it in a floating window"
    }
    required init?(coder: NSCoder) { fatalError() }

    private var font: NSFont { .systemFont(ofSize: 11.5, weight: .medium) }
    private var segW: [CGFloat] { labels.map { ($0.1 as NSString).size(withAttributes: [.font: font]).width + 38 } }
    private func seg(_ i: Int) -> NSRect {
        if i == 2 { return NSRect(x: bounds.width - 3 - 30, y: 3, width: 30, height: bounds.height - 6) }
        let x = 3 + segW.prefix(i).reduce(0, +)
        return NSRect(x: x, y: 3, width: segW[i], height: bounds.height - 6)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseMoved(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let h = (0..<3).first { seg($0).contains(p) }
        if h != hover { hover = h; needsDisplay = true }
    }
    override func mouseExited(with e: NSEvent) { hover = nil; needsDisplay = true }
    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        guard let i = (0..<3).first(where: { seg($0).contains(p) }) else { return }
        if i == 2 { onPopOut?(); return }
        let want = i == 0
        if want != prose { prose = want; onChange?(want) }
    }
    override func draw(_ dirty: NSRect) {
        let c = colors
        let track = bounds.insetBy(dx: 0.5, dy: 0.5)
        // over the editor: a solid mantle under the capsule's tint so text
        // behind it never shows through
        c.mantle.setFill()
        NSBezierPath(roundedRect: track, xRadius: track.height / 2, yRadius: track.height / 2).fill()
        CapsuleStyle.track(track, c)
        // ⤢ pop out
        let pr = seg(2)
        CapsuleStyle.chip(pr, c, on: false, hover: hover == 2)
        ButtonStyle.symbol("rectangle.portrait.and.arrow.right", in: pr, color: hover == 2 ? c.text : c.dim, size: 11)
        for i in 0..<2 {
            let r = seg(i)
            let on = (i == 0) == prose
            CapsuleStyle.chip(r, c, on: on, hover: hover == i)
            let fg = on ? c.text : c.dim
            ButtonStyle.symbol(labels[i].0, in: NSRect(x: r.minX + 8, y: r.minY, width: 14, height: r.height), color: on ? c.accentOn : fg, size: 10)
            let a: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
            let s = labels[i].1 as NSString
            let sz = s.size(withAttributes: a)
            s.draw(at: NSPoint(x: r.minX + 26, y: r.midY - sz.height / 2), withAttributes: a)
        }
    }
}

// The reading page on its own: a lean floating window (Figma B's "present"
// idea) — no chrome but a thin draggable title strip, follows the note as
// it is saved (1 s check), ⌘+ / ⌘− size, Esc / ⌘W close. A non-activating
// panel like the tool panels: opening it never drags the shared window up.
final class ProseWindow: NSPanel {
    private static var open: [ProseWindow] = []
    // true in the `workspace-switcher prose` process: closing the last window ends it
    static var standalone = false
    private let page = ProseView(frame: .zero)
    private let path: String
    private let colors: PopupColors
    private let font: String
    private var size: CGFloat
    private let width: CGFloat
    private var stamp: Date?
    private var timer: Timer?

    static func show(path: String, colors: PopupColors, font: String, size: CGFloat, width: CGFloat) {
        if let w = open.first(where: { $0.path == path }) { w.orderFrontRegardless(); w.makeKey(); return }
        let w = ProseWindow(path: path, colors: colors, font: font, size: size, width: width)
        open.append(w)
        w.orderFrontRegardless()
        w.makeKey()
    }

    private init(path: String, colors: PopupColors, font: String, size: CGFloat, width: CGFloat) {
        self.path = path
        self.colors = colors
        self.font = font
        self.size = size
        self.width = width
        let scr = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1400, height: 900)
        let w = min(scr.width * 0.6, width + 160), h = scr.height * 0.8
        super.init(contentRect: NSRect(x: scr.midX - w / 2, y: scr.midY - h / 2, width: w, height: h),
                   styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                   backing: .buffered, defer: false)
        title = (path as NSString).lastPathComponent
        titlebarAppearsTransparent = true
        titleVisibility = .hidden
        isMovableByWindowBackground = true
        isFloatingPanel = true
        level = .floating
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        backgroundColor = colors.base
        collectionBehavior = [.fullScreenAuxiliary]
        // lean: the themed ✕ below replaces the traffic lights
        for b in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] { standardWindowButton(b)?.isHidden = true }
        page.frame = contentView?.bounds ?? .zero
        page.autoresizingMask = [.width, .height]
        contentView?.addSubview(page)
        page.restoreZoom()
        let x = ProseCloseButton(colors: colors) { [weak self] in self?.close() }
        x.frame = NSRect(x: 12, y: (contentView?.bounds.height ?? h) - 34, width: 22, height: 22)
        x.autoresizingMask = [.minYMargin]
        contentView?.addSubview(x)
        render()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.refreshIfChanged() }
    }

    private func mtime() -> Date? {
        (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
    }
    private func render() {
        stamp = mtime()
        guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return }
        page.show(ProseSource(markdown: text, path: path), colors: colors, font: font, size: size, width: width)
    }
    private func refreshIfChanged() {
        if mtime() != stamp { render() }
    }
    override var canBecomeKey: Bool { true }
    // double-click the top strip = fill the screen / back (the Apple title bar
    // gesture; the page covers the transparent title bar, so catch it here)
    private var restoreFrame: NSRect?
    override func sendEvent(_ e: NSEvent) {
        if e.type == .leftMouseDown, e.clickCount == 2, e.locationInWindow.y > frame.height - 28 {
            if let r = restoreFrame { setFrame(r, display: true, animate: true); restoreFrame = nil }
            else if let scr = screen ?? NSScreen.main {
                restoreFrame = frame
                setFrame(scr.visibleFrame, display: true, animate: true)
            }
            return
        }
        // the pinch never reaches the page's own view reliably: take it here
        if e.type == .magnify { page.magnify(with: e); return }
        if e.type == .smartMagnify { page.smartMagnify(with: e); return }
        super.sendEvent(e)
    }
    override func cancelOperation(_ sender: Any?) { close() }
    override func performKeyEquivalent(with e: NSEvent) -> Bool {
        guard e.modifierFlags.contains(.command) else { return super.performKeyEquivalent(with: e) }
        switch e.charactersIgnoringModifiers ?? "" {
        case "w": close(); return true
        case "p": page.exportPDF(); return true
        case "=", "+": page.zoom(by: 1.1); page.saveZoom(); return true
        case "-": page.zoom(by: 1 / 1.1); page.saveZoom(); return true
        case "0": page.resetZoom(); page.saveZoom(); return true
        case "c": page.web.evaluateJavaScript("document.execCommand('copy')"); return true
        case "a": page.web.evaluateJavaScript("document.execCommand('selectAll')"); return true
        default: return super.performKeyEquivalent(with: e)
        }
    }
    override func keyDown(with e: NSEvent) {
        if e.keyCode == 53 { close() } else { super.keyDown(with: e) }
    }
    override func close() {
        timer?.invalidate()
        timer = nil
        Self.open.removeAll { $0 === self }
        super.close()
        if Self.standalone, Self.open.isEmpty { NSApp.terminate(nil) }
    }
}

// The ✕ top-left: the app header's glyph (ghost chip, danger on hover).
final class ProseCloseButton: NSView {
    private let colors: PopupColors
    private let action: () -> Void
    private var hover = false { didSet { needsDisplay = true } }
    init(colors: PopupColors, action: @escaping () -> Void) {
        self.colors = colors
        self.action = action
        super.init(frame: .zero)
        toolTip = "Close (Esc)"
    }
    required init?(coder: NSCoder) { fatalError() }
    override var mouseDownCanMoveWindow: Bool { false }
    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }
    override func mouseEntered(with e: NSEvent) { hover = true }
    override func mouseExited(with e: NSEvent) { hover = false }
    override func mouseDown(with e: NSEvent) {}
    override func mouseUp(with e: NSEvent) { if bounds.contains(convert(e.locationInWindow, from: nil)) { action() } }
    override func draw(_ dirty: NSRect) {
        let dot = bounds.insetBy(dx: 3, dy: 3)
        let fg: NSColor
        if hover {
            let danger = colors.tone(.danger)
            danger.setFill()
            NSBezierPath(ovalIn: dot).fill()
            fg = ButtonStyle.readable(on: danger, preferred: colors.crust)
        } else {
            ButtonStyle.draw(dot, .idle, colors, radius: dot.height / 2, flat: true)
            fg = colors.dim
        }
        let r: CGFloat = 3.2
        let x = NSBezierPath()
        x.move(to: NSPoint(x: dot.midX - r, y: dot.midY - r)); x.line(to: NSPoint(x: dot.midX + r, y: dot.midY + r))
        x.move(to: NSPoint(x: dot.midX + r, y: dot.midY - r)); x.line(to: NSPoint(x: dot.midX - r, y: dot.midY + r))
        x.lineWidth = 1.6
        x.lineCapStyle = .round
        fg.setStroke()
        x.stroke()
    }
}

// Pop-out as its OWN PROCESS (`workspace-switcher prose …`): the page has no
// tie to the app that opened it — move it, keep it when the daemon restarts.
// Colors travel as hex on the command line; one process per file.
enum ProseProcess {
    private static var children: [String: Process] = [:]

    private static func hex(_ c: NSColor) -> String {
        let s = c.usingColorSpace(.sRGB) ?? c
        return String(format: "%02X%02X%02X%02X", Int(round(s.redComponent * 255)), Int(round(s.greenComponent * 255)),
                      Int(round(s.blueComponent * 255)), Int(round(s.alphaComponent * 255)))
    }
    private static func color(_ h: Substring) -> NSColor? {
        guard h.count == 8, let v = UInt32(h, radix: 16) else { return nil }
        return NSColor(srgbRed: CGFloat(v >> 24) / 255, green: CGFloat((v >> 16) & 255) / 255,
                       blue: CGFloat((v >> 8) & 255) / 255, alpha: CGFloat(v & 255) / 255)
    }

    static func launch(path: String, colors c: PopupColors, font: String, size: CGFloat, width: CGFloat) {
        if let p = children[path], p.isRunning {
            NSRunningApplication(processIdentifier: p.processIdentifier)?.activate(options: [.activateIgnoringOtherApps])
            return
        }
        guard let exe = Bundle.main.executablePath else { return }
        let all = [c.background, c.border, c.text, c.dim, c.highlight, c.accent] + c.palette.all
        let p = Process()
        p.executableURL = URL(fileURLWithPath: exe)
        p.arguments = ["prose", "--colors", all.map(hex).joined(separator: ","), "--font", font,
                       "--size", String(Double(size)), "--width", String(Double(width)), path]
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        do { try p.run(); children[path] = p } catch { ProseWindow.show(path: path, colors: c, font: font, size: size, width: width) }
    }

    // the child's entry (main.swift): never returns
    static func run(_ args: [String]) -> Never {
        var colors = PopupColors(), font = "", size: CGFloat = 19, width: CGFloat = 900, path = ""
        var i = 0
        while i < args.count {
            switch args[i] {
            case "--colors" where i + 1 < args.count:
                let cs = args[i + 1].split(separator: ",").compactMap(color)
                if cs.count == 11, let pal = PopupPalette(Array(cs[6...])) {
                    colors = PopupColors(background: cs[0], border: cs[1], text: cs[2], dim: cs[3],
                                         highlight: cs[4], accent: cs[5], palette: pal)
                }
                i += 1
            case "--font" where i + 1 < args.count: font = args[i + 1]; i += 1
            case "--size" where i + 1 < args.count: size = CGFloat(Double(args[i + 1]) ?? 19); i += 1
            case "--width" where i + 1 < args.count: width = CGFloat(Double(args[i + 1]) ?? 900); i += 1
            default: path = args[i]
            }
            i += 1
        }
        guard FileManager.default.fileExists(atPath: path) else {
            FileHandle.standardError.write(Data("usage: workspace-switcher prose [--colors …] [--font F] [--size N] [--width N] FILE.md\n".utf8))
            exit(2)
        }
        let app = NSApplication.shared
        // a REGULAR app, like the daemon (15015cf): it owns the menu bar while
        // active. An accessory app can't — revealing the auto-hidden menu bar
        // activated the last regular app and took the page's focus with it.
        app.setActivationPolicy(.regular)
        let menu = NSMenu()
        let item = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "")
        appMenu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.submenu = appMenu
        menu.addItem(item)
        app.mainMenu = menu
        ProseWindow.standalone = true
        ProseWindow.show(path: path, colors: colors, font: font, size: size, width: width)
        app.activate(ignoringOtherApps: true)
        app.run()
        exit(0)
    }
}
