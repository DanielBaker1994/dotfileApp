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
        if let html = RichText.pandocHTML(md), !html.isEmpty { return html }
        return basic(md)
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
final class ProseView: NSView {
    let web: WKWebView
    var onLinkClick: ((URL) -> Void)?
    private var lastPath = ""
    private var gen = 0

    override init(frame: NSRect) {
        let cfg = WKWebViewConfiguration()
        web = WKWebView(frame: frame, configuration: cfg)
        super.init(frame: frame)
        web.autoresizingMask = [.width, .height]
        web.frame = bounds
        web.setValue(false, forKey: "drawsBackground")
        addSubview(web)
        wantsLayer = true
    }
    required init?(coder: NSCoder) { fatalError() }

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

// Prose | Edit — a small capsule switch at the editor's bottom right (the
// view switcher's look: a track with the current mode on a raised chip)
final class ProseModeSwitch: NSView {
    var colors: PopupColors { didSet { needsDisplay = true } }
    var prose = false { didSet { needsDisplay = true } }
    var editLabel = "Edit"
    var onChange: ((Bool) -> Void)?
    private var hover: Int?
    private var tracking: NSTrackingArea?
    private let labels: [(String, String)]
    override var isFlipped: Bool { true }

    init(colors: PopupColors, editLabel: String) {
        self.colors = colors
        self.editLabel = editLabel
        labels = [("text.alignleft", "Prose"), ("chevron.left.forwardslash.chevron.right", editLabel)]
        super.init(frame: NSRect(x: 0, y: 0, width: 150, height: 28))
        frame.size.width = segW.reduce(6, +)
        toolTip = "Reading view ⌘⇧P — Esc goes back to the editor"
    }
    required init?(coder: NSCoder) { fatalError() }

    private var font: NSFont { .systemFont(ofSize: 11.5, weight: .medium) }
    private var segW: [CGFloat] { labels.map { ($0.1 as NSString).size(withAttributes: [.font: font]).width + 38 } }
    private func seg(_ i: Int) -> NSRect {
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
        let h = (0..<2).first { seg($0).contains(p) }
        if h != hover { hover = h; needsDisplay = true }
    }
    override func mouseExited(with e: NSEvent) { hover = nil; needsDisplay = true }
    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        guard let i = (0..<2).first(where: { seg($0).contains(p) }) else { return }
        let want = i == 0
        if want != prose { prose = want; onChange?(want) }
    }
    override func draw(_ dirty: NSRect) {
        let c = colors
        let track = bounds.insetBy(dx: 0.5, dy: 0.5)
        c.mantle.setFill()
        NSBezierPath(roundedRect: track, xRadius: track.height / 2, yRadius: track.height / 2).fill()
        c.text.withAlphaComponent(0.08).setStroke()
        NSBezierPath(roundedRect: track, xRadius: track.height / 2, yRadius: track.height / 2).stroke()
        for i in 0..<2 {
            let r = seg(i)
            let on = (i == 0) == prose
            let chip = NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2)
            if on {
                NSGraphicsContext.saveGraphicsState()
                let sh = NSShadow()
                sh.shadowColor = NSColor.black.withAlphaComponent(c.isLight ? 0.15 : 0.3)
                sh.shadowOffset = NSSize(width: 0, height: -1)
                sh.shadowBlurRadius = 2
                sh.set()
                (c.isLight ? NSColor.white : c.surface1).setFill()
                chip.fill()
                NSGraphicsContext.restoreGraphicsState()
            } else if hover == i {
                c.text.withAlphaComponent(0.06).setFill()
                chip.fill()
            }
            let fg = on ? c.text : c.dim
            ButtonStyle.symbol(labels[i].0, in: NSRect(x: r.minX + 8, y: r.minY, width: 14, height: r.height), color: on ? c.accentOn : fg, size: 10)
            let a: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
            let s = labels[i].1 as NSString
            let sz = s.size(withAttributes: a)
            s.draw(at: NSPoint(x: r.minX + 26, y: r.midY - sz.height / 2), withAttributes: a)
        }
    }
}
