import AppKit
import WebKit

// Notes prose mode: the current note as a READING page that renders the
// SAME document the PDF export produces — pandoc `-s -f gfm -t html5` with
// `[notes] pdf-css` (else the built-in light style) as its header, so what
// you read is what you export — in a centred column of `prose-width`
// points. The switch at the editor's bottom right (Prose | Edit), ⌘⇧P or
// Esc goes back to the editor (nvim / native).
//
// pandoc missing → `ProseRender.basic` (headings, lists + task boxes,
// fences, quotes, rules, paragraphs, inline code / bold / italic / links /
// images) inside the same header style.

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
        var fenceLang = ""
        func pre(_ lines: [String]) -> String {
            let cls = fenceLang.isEmpty ? "" : " class=\"\(esc(fenceLang))\""
            return "<pre\(cls)><code>" + esc(lines.joined(separator: "\n")) + "</code></pre>"
        }
        func flushPara() {
            if !para.isEmpty { out.append("<p>" + para.map(inline).joined(separator: " ") + "</p>"); para = [] }
        }
        func closeList() { if let l = list { out.append("</\(l)>"); list = nil } }
        for line in md.components(separatedBy: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if var f = fence {
                if t.hasPrefix("```") { out.append(pre(f)); fence = nil }
                else { f.append(line); fence = f }
                continue
            }
            if t.hasPrefix("```") {
                flushPara(); closeList(); fence = []
                fenceLang = String(t.dropFirst(3)).trimmingCharacters(in: .whitespaces)
                    .split(separator: " ").first.map(String.init) ?? ""
                continue
            }
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
        if let f = fence { out.append(pre(f)) }
        flushPara(); closeList()
        return out.joined(separator: "\n")
    }

    // The dark theme layer over the PDF style: its layout, recolored to the
    // active theme (the original Prose look) instead of the light print
    // palette. Applied to BOTH the reading view and the PDF export.
    static func rgba(_ x: NSColor, _ a: CGFloat) -> String {
        let s = ButtonStyle.opaque(x)
        return String(format: "rgba(%d,%d,%d,%.3f)",
                      Int(s.redComponent * 255), Int(s.greenComponent * 255), Int(s.blueComponent * 255), a)
    }

    static func themeCSS(_ c: PopupColors) -> String {
        let bg = css(c.base), text = css(c.text), dim = css(c.dim)
        let accent = css(c.accentOn), accent2 = css(c.tone(.accent2))
        let well = css(c.mantle), s0 = css(c.surface0), s1 = css(c.surface1)
        let rule = css(ButtonStyle.opaque(c.dim).blended(withFraction: 0.6, of: ButtonStyle.opaque(c.base)) ?? c.dim)
        func tint(_ x: NSColor) -> String { rgba(x, 0.12) }
        let vars: [(String, String)] = [
            ("bg", bg), ("text", text), ("dim", dim), ("accent", accent), ("accent2", accent2),
            ("well", well), ("s0", s0), ("s1", s1), ("rule", rule),
            ("info", css(c.tone(.info))), ("info-tint", tint(c.tone(.info))),
            ("success", css(c.tone(.success))), ("success-tint", tint(c.tone(.success))),
            ("accent2-tint", tint(c.tone(.accent2))),
            ("warning", css(c.tone(.warning))), ("warning-tint", tint(c.tone(.warning))),
            ("danger", css(c.tone(.danger))), ("danger-tint", tint(c.tone(.danger))),
            ("selection", rgba(c.accentOn, 0.28)),
        ]
        let root = ":root { " + vars.map { "--p-\($0.0): \($0.1);" }.joined(separator: " ") + " }"
        let file = (try? String(contentsOfFile: assetDir + "/prose_theme.css", encoding: .utf8)) ?? ""
        return "<style>\n" + root + "\n" + file + "\n</style>"
    }

    // pandoc's built-in `tango` is a LIGHT theme: its keywords/types are dark
    // ink (#204a87 ≈ 2:1) and its variables are black (#000 ≈ 1.2:1) on the
    // dark page the theme layer paints, so every language reads as the same
    // smudge. Pick a highlight theme that reads on the active page — dark
    // pages get a dark theme (distinct bright tokens), light pages keep tango.
    // `[notes] pdf-highlight` still wins when the user names a theme.
    static func highlightTheme(_ c: PopupColors) -> String {
        c.isLight ? "tango" : "breezedark"
    }

    // Screen-only copy-to-clipboard buttons (code blocks only — tables have none): the reading view's JS wraps each
    // <pre> in .codeblock and adds a .copy-btn to its left; this is the flex
    // layout + hover/copied states. Never reaches the PDF (export() skips the
    // reading page entirely, and weasyprint runs no JS anyway).
    static func copyButtonCSS(_ c: PopupColors) -> String {
        """
        .codeblock { display: flex; align-items: stretch; }
        .codeblock pre { flex: 1 1 auto; min-width: 0; }
        .copy-btn { flex: 0 0 auto; align-self: stretch; width: 34px; margin: .8em 8px .8em 0;
          display: flex; align-items: center; justify-content: center; padding: 0;
          border: none; border-radius: 8px; background: var(--p-s0, \(css(c.surface0))); color: var(--p-dim, \(css(c.dim)));
          cursor: pointer; transition: background .15s, color .15s; }
        .copy-btn:hover { background: var(--p-s1, \(css(c.surface1))); color: var(--p-text, \(css(c.text))); }
        .copy-btn:active { background: var(--p-s1, \(css(c.surface1))); }
        .copy-btn.copied { background: var(--p-success-tint, \(rgba(c.tone(.success), 0.20))); color: var(--p-success, \(css(c.tone(.success)))); }
        .copy-btn:focus { outline: none; }
        .copy-btn svg { width: 16px; height: 16px; }
        """
    }

    // The reading page: the SAME document the PDF export produces — pandoc
    // `-s -f gfm -t html5` with `[notes] pdf-css` (else the built-in style)
    // as its header — so what you read is what you export. The dark theme
    // layer (themeCSS) recolors it; the screen adds only a <base> for
    // relative images and the `prose-width` column.
    static func page(_ src: ProseSource, colors c: PopupColors, font: String, size: CGFloat, width: CGFloat) -> String {
        var cfg = ProsePDF.Config()
        cfg.pandoc = RichText.pandocBin
        func notes(_ k: String) -> String? {
            configSectionValue("notes", k).map { $0.trimmingCharacters(in: .whitespaces) }.flatMap { $0.isEmpty ? nil : $0 }
        }
        if let v = notes("pdf-css") { cfg.css = v }
        if let v = notes("pdf-filter") { cfg.filter = v }
        if let v = notes("pdf-highlight") { cfg.highlight = v }
        else { cfg.highlight = highlightTheme(c) }   // legible tokens on this theme
        cfg.themeCSS = themeCSS(c)
        var html = ProsePDF.screenHTML(note: src.path, cfg) ?? shell(src, cfg)
        // <base> so relative images resolve + the screen-only column width
        let base = URL(fileURLWithPath: (src.path as NSString).deletingLastPathComponent, isDirectory: true).absoluteString
        var override = "<base href=\"\(esc(base))\">\n<style>\n"
            + "body { max-width: \(Int(width))px !important; }\n"
            + "</style>"
        // [notes] copy-buttons (default true): the style id is the switch the
        // reading view's script checks — screen-only, so the PDF never has it
        if tri(notes("copy-buttons")) ?? true {
            override += "\n<style id=\"ws-copy\">\n" + copyButtonCSS(c) + "</style>"
        }
        if let head = html.range(of: "</head>", options: .caseInsensitive) {
            html.insert(contentsOf: override, at: head.lowerBound)
        }
        return html
    }

    // pandoc missing: the built-in renderer inside the same header style
    private static func shell(_ src: ProseSource, _ c: ProsePDF.Config) -> String {
        """
        <!doctype html><html><head><meta charset="utf-8">
        \(ProsePDF.cssContent(c))
        </head><body>\(basic(src.markdown))</body></html>
        """
    }
}

// the reading page itself (a WKWebView painted in the card color)
final class ProseView: NSView, WKScriptMessageHandler {
    let web: WKWebView
    func scrollBy(_ dy: Int) { web.evaluateJavaScript("window.scrollBy(0, \(dy))") }
    var onLinkClick: ((URL) -> Void)?
    var onOpenImage: ((String) -> Void) = { FilePopup.show(path: $0, over: nil) }
    private var lastPath = ""
    private var gen = 0
    private var themeColors = PopupColors()   // the PDF export's theme layer
    private var lastKey = ""                   // path+content+size+theme: skip no-op renders
    private var loadedPath: String?            // the note the web view currently holds
    private var loadedHeadKey = ""             // path+width+theme: the head CSS the page holds
    // one scratch file per view: the in-editor view and pop-out windows (and
    // other processes) must never write each other's page
    private let pageFile = URL(fileURLWithPath: NSHomeDirectory()
        + "/.cache/kitchen-sink/prose/page-\(UUID().uuidString).html")

    // Copy-to-clipboard for code blocks (ported from the dotfiles'
    // markdown_generator/copy_button.js): wrap each <pre> (and table) in a flex
    // .codeblock and put a .copy-btn to its left. Screen-only — the PDF path
    // never loads this (weasyprint runs no JS). The page opts in by emitting
    // <style id="ws-copy"> ([notes] copy-buttons); the function is re-run after
    // an in-place body patch, which would otherwise drop the buttons.
    static let copyButtonJS = """
        (function () {
          var COPY = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>';
          var CHECK = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>';
          function addCopyButtons() {
            if (!document.getElementById('ws-copy')) return;
            document.querySelectorAll('pre').forEach(function (pre) {
              if (pre.parentElement && pre.parentElement.classList.contains('codeblock')) return;
              var code = pre.querySelector('code') || pre;
              var btn = document.createElement('button');
              btn.className = 'copy-btn';
              btn.type = 'button';
              btn.innerHTML = COPY;
              btn.setAttribute('aria-label', 'Copy code to clipboard');
              btn.addEventListener('click', function () {
                var text = code.innerText;
                function done(ok) {
                  btn.classList.toggle('copied', ok);
                  btn.innerHTML = ok ? CHECK : COPY;
                  setTimeout(function () { btn.innerHTML = COPY; btn.classList.remove('copied'); }, 1500);
                }
                function fallback() {
                  var ta = document.createElement('textarea');
                  ta.value = text; ta.style.position = 'fixed'; ta.style.opacity = '0';
                  document.body.appendChild(ta); ta.select();
                  var ok = false;
                  try { ok = document.execCommand('copy'); } catch (e) {}
                  document.body.removeChild(ta); done(ok);
                }
                if (navigator.clipboard && navigator.clipboard.writeText) {
                  navigator.clipboard.writeText(text).then(function () { done(true); }, fallback);
                } else { fallback(); }
              });
              var wrap = document.createElement('div');
              wrap.className = 'codeblock';
              pre.parentNode.insertBefore(wrap, pre);
              wrap.appendChild(btn);
              wrap.appendChild(pre);
            });
          }
          window.__wsAddCopyButtons = addCopyButtons;
          if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', addCopyButtons);
          else addCopyButtons();
        })();
        """

    override init(frame: NSRect) {
        let cfg = WKWebViewConfiguration()
        // double-click a picture -> its full-size popup (like the nvim view)
        cfg.userContentController.addUserScript(WKUserScript(source: """
            document.addEventListener('dblclick', function (e) {
              var t = e.target;
              if (t && t.tagName === 'IMG') window.webkit.messageHandlers.wsImage.postMessage(t.src);
            });
            """, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        cfg.userContentController.addUserScript(WKUserScript(source: Self.copyButtonJS,
                                                             injectionTime: .atDocumentEnd, forMainFrameOnly: true))
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
        if let v = notes("pdf-filter") { c.filter = v }
        if let v = notes("pdf-path") { c.outDir = v }
        if let v = notes("pdf-highlight") { c.highlight = v }
        else { c.highlight = ProseRender.highlightTheme(themeColors) }
        c.themeCSS = ProseRender.themeCSS(themeColors)
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

    // Render off the main thread (pandoc). An unchanged render is dropped; a
    // same-note update patches the document body IN PLACE (no reload, no
    // flash, scroll kept); a full reload only for a new note or the first paint.
    func show(_ src: ProseSource, colors: PopupColors, font: String, size: CGFloat, width: CGFloat) {
        themeColors = colors
        layer?.backgroundColor = colors.base.cgColor
        let sig = ProseRender.css(colors.base) + ProseRender.css(colors.text) + ProseRender.css(colors.accentOn)
        // the copy-button style lives in the <head>: fold its config into both
        // keys so toggling [notes] copy-buttons reloads the page
        let copy = tri(configSectionValue("notes", "copy-buttons")) ?? true
        let key = "\(src.path)\u{1}\(Int(size))\u{1}\(font)\u{1}\(Int(width))\u{1}\(sig)\u{1}\(copy)\u{1}\(src.markdown)"
        guard key != lastKey else { return }   // nothing changed: keep what is on screen
        // The <head> (themeCSS, <base>, the column width) is what colors the
        // page; only patch the body in place when the head is unchanged too.
        let headKey = "\(src.path)\u{1}\(Int(width))\u{1}\(sig)\u{1}\(copy)"
        let same = src.path == loadedPath
        let sameHead = same && headKey == loadedHeadKey
        lastKey = key
        lastPath = src.path
        gen += 1
        let g = gen
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let html = ProseRender.page(src, colors: colors, font: font, size: size, width: width)
            DispatchQueue.main.async {
                guard let self, g == self.gen else { return }
                if sameHead, let body = Self.bodyInner(html) {
                    self.web.evaluateJavaScript(Self.patchJS(body))
                    return
                }
                let dir = NSHomeDirectory() + "/.cache/kitchen-sink/prose"
                try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
                let file = self.pageFile
                guard (try? Data(html.utf8).write(to: file)) != nil else { return }
                self.loadedPath = src.path
                self.loadedHeadKey = headKey
                if same {   // re-loading the same note: keep the scroll spot
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

    // the document's <body> content (nil when absent) — for an in-place update
    private static func bodyInner(_ html: String) -> String? {
        guard let b = html.range(of: "<body", options: .caseInsensitive),
              let gt = html[b.upperBound...].firstIndex(of: ">"),
              let e = html.range(of: "</body>", options: .caseInsensitive) else { return nil }
        let start = html.index(after: gt)
        guard start <= e.lowerBound else { return nil }
        return String(html[start..<e.lowerBound])
    }

    // replace the body in place, keeping the scroll position (no reload)
    private static func patchJS(_ body: String) -> String {
        let lit = (try? JSONSerialization.data(withJSONObject: body, options: .fragmentsAllowed))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "\"\""
        return "var __y=window.scrollY;document.body.innerHTML=\(lit);window.scrollTo(0,__y);"
            + "window.__wsAddCopyButtons&&window.__wsAddCopyButtons();"
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
    var onStyle: ((NSRect) -> Void)?     // the style chip: the document template menu (rect = the chip, own coords)
    private var hover: Int?
    private var tracking: NSTrackingArea?
    private let labels: [(String, String)]
    override var isFlipped: Bool { true }

    init(colors: PopupColors, editLabel: String) {
        self.colors = colors
        self.editLabel = editLabel
        labels = [("text.alignleft", "Prose"), ("chevron.left.forwardslash.chevron.right", editLabel)]
        super.init(frame: NSRect(x: 0, y: 0, width: 150, height: 28))
        frame.size.width = segW.reduce(6, +) + 34 + 32
        toolTip = "Reading view ⌘⇧P — Esc goes back to the editor; ◐ picks a document style; ⤢ opens it in a floating window"
    }
    required init?(coder: NSCoder) { fatalError() }

    private var font: NSFont { .systemFont(ofSize: 11.5, weight: .medium) }
    private var segW: [CGFloat] { labels.map { ($0.1 as NSString).size(withAttributes: [.font: font]).width + 38 } }
    private func seg(_ i: Int) -> NSRect {
        if i == 2 { return NSRect(x: bounds.width - 3 - 30, y: 3, width: 30, height: bounds.height - 6) }
        if i == 3 { return NSRect(x: bounds.width - 3 - 30 - 32, y: 3, width: 30, height: bounds.height - 6) }
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
        let h = (0..<4).first { seg($0).contains(p) }
        if h != hover { hover = h; needsDisplay = true }
    }
    override func mouseExited(with e: NSEvent) { hover = nil; needsDisplay = true }
    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        guard let i = (0..<4).first(where: { seg($0).contains(p) }) else { return }
        if i == 2 { onPopOut?(); return }
        if i == 3 { onStyle?(seg(3)); return }
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
        // ◐ document style
        let sr = seg(3)
        CapsuleStyle.chip(sr, c, on: false, hover: hover == 3)
        ButtonStyle.symbol("paintpalette", in: sr, color: hover == 3 ? c.text : c.dim, size: 11)
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
// it is saved (debounced mtime check; the body is patched in place, no
// reload), ⌘+ / ⌘− size, Esc / ⌘W close.
//
// A plain NSWindow at NORMAL level — deliberately NOT a floating NSPanel.
// JankyBorders only borders windows the window server tags as documents
// (`window_suitable` in JankyBorders: document tag, or floating+modal). Any
// window at a non-normal level, and any NSPanel, is tagged floating and
// skipped — which is why the focused reading ("PDF") window had no highlight
// border. A normal document window is also what AeroSpace's on-window-detected
// tiling rule for this app expects.
final class ProseWindow: NSWindow {
    private static var open: [ProseWindow] = []
    // true in the `kitchen-sink prose` process: closing the last window ends it
    static var standalone = false
    private let page = ProseView(frame: .zero)
    private let path: String
    private let colors: PopupColors
    private let font: String
    private var size: CGFloat
    private let width: CGFloat
    private var stamp: Date?
    private var timer: Timer?
    private var pending: Timer?

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
        // normal level / a document window: see the class comment. JankyBorders
        // skips floating-level windows and NSPanels, so this must stay normal.
        level = .normal
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
        // follow the note: poll its mtime, but coalesce a burst of autosaves
        // into ONE render a beat after the writes stop (no reload per keystroke)
        timer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { [weak self] _ in self?.poll() }
    }

    private func mtime() -> Date? {
        (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
    }
    private func poll() {
        guard mtime() != stamp else { return }
        pending?.invalidate()
        pending = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: false) { [weak self] _ in self?.render() }
    }
    private func render() {
        stamp = mtime()
        let p = path
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let text = try? String(contentsOfFile: p, encoding: .utf8) else { return }
            DispatchQueue.main.async {
                guard let self else { return }
                self.page.show(ProseSource(markdown: text, path: p), colors: self.colors,
                               font: self.font, size: self.size, width: self.width)
            }
        }
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
        // j / k scroll the page (the web view would swallow them as plain keys)
        if e.type == .keyDown, e.modifierFlags.intersection([.command, .control, .option]).isEmpty,
           let c = e.charactersIgnoringModifiers, c == "j" || c == "k" {
            page.scrollBy(c == "j" ? 60 : -60)
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
        pending?.invalidate()
        pending = nil
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

// Pop-out as its OWN PROCESS (`kitchen-sink prose …`): the page has no
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
            FileHandle.standardError.write(Data("usage: kitchen-sink prose [--colors …] [--font F] [--size N] [--width N] FILE.md\n".utf8))
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
