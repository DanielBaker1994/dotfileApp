import Foundation

// Prose ▸ Export PDF: the note through the owner's dotfiles recipe —
// `pandoc -s -f gfm -t html5 --syntax-highlighting=tango
// --include-in-header=CSS` → `weasyprint` — into `[notes] pdf-path`
// (default ~/Downloads) as NOTE-STEM.pdf (a re-export overwrites it).
// Foundation only (`bin/run-tests.sh prose`); the caller copies the path
// and shows the toast (`ProseView.exportPDF`).
enum ProsePDF {
    struct Config {
        var pandoc = "/opt/homebrew/bin/pandoc"
        var engine = "/opt/homebrew/bin/weasyprint"   // [notes] pdf-engine-bin
        var css = ""                                   // [notes] pdf-css: a <style> header snippet; "" = built in
        var outDir = "~/Downloads"                     // [notes] pdf-path
        var highlight = "tango"                        // [notes] pdf-highlight
        var filter = ""                                // [notes] pdf-filter: Lua filters, comma list; "" = diagrams.lua beside pdf-css; "none" = off
        var themeCSS = ""                              // dark theme layer (ProseRender.themeCSS)
        var cacheDir = NSHomeDirectory() + "/.cache/kitchen-sink/prose"
    }

    static func expand(_ p: String) -> String { (p as NSString).expandingTildeInPath }

    // A unique scratch file under cacheDir: concurrent renders (the in-editor
    // view + floating windows, in one process or across processes) must never
    // share one path, or they swap each other's HTML.
    private static func scratchFile(_ c: Config, _ prefix: String) -> String {
        (c.cacheDir as NSString).appendingPathComponent("\(prefix)-\(UUID().uuidString).html")
    }

    // Remove a scratch file — but never the path the user configured:
    // headerFile returns `[notes] pdf-css` itself when there is no theme layer.
    private static func removeScratch(_ path: String, _ c: Config) {
        guard path.hasPrefix(c.cacheDir) else { return }
        try? FileManager.default.removeItem(atPath: path)
    }

    static func outputPath(note: String, _ c: Config) -> String {
        let stem = ((note as NSString).lastPathComponent as NSString).deletingPathExtension
        return (expand(c.outDir) as NSString).appendingPathComponent((stem.isEmpty ? "note" : stem) + ".pdf")
    }

    // the pandoc Lua filters: `[notes] pdf-filter` (comma list), else diagrams.lua
    // in the folder of `pdf-css` (```dot / ```d2 → inline SVG); only files that exist
    static func filterPaths(_ c: Config) -> [String] {
        let want = c.filter.trimmingCharacters(in: .whitespaces)
        if want.lowercased() == "none" { return [] }
        var list = want.split(separator: ",").map { expand($0.trimmingCharacters(in: .whitespaces)) }
        if list.isEmpty, !c.css.isEmpty {
            list = [(expand(c.css) as NSString).deletingLastPathComponent + "/diagrams.lua"]
        }
        return list.filter { FileManager.default.fileExists(atPath: $0) }
    }

    static func pandocArgs(note: String, css: String, html: String, _ c: Config, sourcepos: Bool = false) -> [String] {
        let dir = (note as NSString).deletingLastPathComponent
        let stem = ((note as NSString).lastPathComponent as NSString).deletingPathExtension
        let filters = (sourcepos ? [sourceposFilterPath(c)] : []) + filterPaths(c)
        return ["-s", "-f", sourcepos ? "gfm+sourcepos" : "gfm", "-t", "html5", "--syntax-highlighting=\(c.highlight.isEmpty ? "tango" : c.highlight)",
                "-V", "lang=en", "--metadata", "pagetitle=\(stem)", "--resource-path=\(dir)",
                "--include-in-header=\(css)",
                // the header file holds the app theme's --p-* palette: diagrams.lua colors a
                // document with no style marker from it
                "--metadata=ws-header=\(css)"] + filters.map { "--lua-filter=\($0)" } + ["-o", html, note]
    }

    // gfm+sourcepos (the reading view, for the nvim position sync) wraps
    // things so the HTML no longer matches what plain gfm (Export PDF) makes;
    // this filter undoes it (`bin/run-tests.sh prose` renders every markdown
    // snippet both ways and compares the browser DOMs):
    // - a `- [ ] todo` item is split into Plain{☐} + a wrapper Div holding the
    //   text, so the writer misses the task item (a literal ☐, the text on its
    //   own line) → the box goes back in front of the text;
    // - every inline raw HTML tag gets its own position <span>, so
    //   `<kbd>Cmd</kbd>` / `<span class="badge ok">…</span>` close at once in
    //   the browser (an empty pill, plain text after it) → those spans go.
    static let sourceposFilter = """
    function Span(el)
      if el.attributes.wrapper == "1" and #el.content > 0 then
        for _, x in ipairs(el.content) do
          if x.t ~= "RawInline" then return nil end
        end
        return el.content
      end
    end

    local box = { ["☐"] = true, ["☒"] = true }
    function BulletList(el)
      for i, item in ipairs(el.content) do
        local a, b = item[1], item[2]
        if a and b and a.t == "Plain" and #a.content == 1 and a.content[1].t == "Str"
            and box[a.content[1].text] and b.t == "Div" and b.content[1]
            and (b.content[1].t == "Plain" or b.content[1].t == "Para") then
          local first = b.content[1]
          local inl = pandoc.List({ a.content[1], pandoc.Space() })
          inl:extend(first.content)
          first.content = inl
          local blocks = pandoc.List({ first })
          for j = 2, #b.content do blocks:insert(b.content[j]) end
          for j = 3, #item do blocks:insert(item[j]) end
          el.content[i] = blocks
        end
      end
      return el
    end
    """

    // the filter as a file in cacheDir (rewritten only when its text changed)
    static func sourceposFilterPath(_ c: Config) -> String {
        let path = (c.cacheDir as NSString).appendingPathComponent("sourcepos-fix.lua")
        let data = Data(sourceposFilter.utf8)
        if FileManager.default.contents(atPath: path) != data {
            try? FileManager.default.createDirectory(atPath: c.cacheDir, withIntermediateDirectories: true)
            try? data.write(to: URL(fileURLWithPath: path), options: .atomic)
        }
        return path
    }

    static func engineArgs(note: String, html: String, out: String) -> [String] {
        let base = URL(fileURLWithPath: (note as NSString).deletingLastPathComponent, isDirectory: true).absoluteString
        // --pdf-tags: a tagged PDF (structure tree: headings, paragraphs, lists,
        // tables) so readers / Word / Acrobat can reflow and copy by paragraph
        return ["--pdf-tags", "-u", base, html, out]
    }

    // the header CSS file: the configured one, else the built-in style, with
    // the dark theme layer appended when set
    static func headerFile(_ c: Config) throws -> String {
        let own = expand(c.css)
        let ownExists = !c.css.isEmpty && FileManager.default.fileExists(atPath: own)
        if ownExists && c.themeCSS.isEmpty { return own }
        var text = (ownExists ? (try? String(contentsOfFile: own, encoding: .utf8)) : nil) ?? builtinCSS
        if !c.themeCSS.isEmpty { text += "\n" + c.themeCSS }
        let f = scratchFile(c, "header")
        try Data(text.utf8).write(to: URL(fileURLWithPath: f))
        return f
    }

    // the header CSS text: the configured file, else the built-in style, with
    // the dark theme layer appended when set
    static func cssContent(_ c: Config) -> String {
        let own = expand(c.css)
        var text = (!c.css.isEmpty ? (try? String(contentsOfFile: own, encoding: .utf8)) : nil) ?? builtinCSS
        if !c.themeCSS.isEmpty { text += "\n" + c.themeCSS }
        return text
    }

    // The SAME pandoc → HTML5 document the PDF export feeds to weasyprint,
    // returned for the reading view so what you read is what you export.
    // nil = pandoc missing / failed (the caller falls back to the built-in
    // renderer).
    static func screenHTML(note: String, _ c: Config) -> String? {
        let fm = FileManager.default
        guard fm.isExecutableFile(atPath: expand(c.pandoc)), fm.fileExists(atPath: note) else { return nil }
        do {
            try fm.createDirectory(atPath: c.cacheDir, withIntermediateDirectories: true)
            let css = try headerFile(c)
            let html = scratchFile(c, "prose")
            defer { removeScratch(css, c); removeScratch(html, c) }
            let p = try runProcess(expand(c.pandoc), pandocArgs(note: note, css: css, html: html, c, sourcepos: true))
            guard p.code == 0 else { return nil }
            return try? String(contentsOfFile: html, encoding: .utf8)
        } catch { return nil }
    }

    // runs synchronously (call it off main): the PDF's path, or why not
    static func export(note: String, _ c: Config) -> Result<String, ExportError> {
        let fm = FileManager.default
        guard fm.isExecutableFile(atPath: expand(c.pandoc)) else {
            return .failure(ExportError("pandoc not found — brew install pandoc"))
        }
        guard fm.isExecutableFile(atPath: expand(c.engine)) else {
            return .failure(ExportError("weasyprint not found — brew install weasyprint"))
        }
        guard fm.fileExists(atPath: note) else { return .failure(ExportError("note not found: \(note)")) }
        let out = outputPath(note: note, c)
        do {
            try fm.createDirectory(atPath: c.cacheDir, withIntermediateDirectories: true)
            try fm.createDirectory(atPath: (out as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
            let css = try headerFile(c)
            let html = scratchFile(c, "pdf")
            defer { removeScratch(css, c); removeScratch(html, c) }
            let p = try runProcess(expand(c.pandoc), pandocArgs(note: note, css: css, html: html, c))
            guard p.code == 0 else { return .failure(ExportError("pandoc failed: " + firstLine(p.err))) }
            let w = try runProcess(expand(c.engine), engineArgs(note: note, html: html, out: out))
            guard w.code == 0, fm.fileExists(atPath: out) else {
                return .failure(ExportError("weasyprint failed: " + firstLine(w.err)))
            }
            return .success(out)
        } catch {
            return .failure(ExportError(error.localizedDescription))
        }
    }

    struct ExportError: Error, CustomStringConvertible {
        let description: String
        init(_ s: String) { description = s }
    }

    private static func firstLine(_ s: String) -> String {
        s.split(separator: "\n").first(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty })
            .map(String.init) ?? "exit status"
    }

    // a light document style for when [notes] pdf-css is unset
    static let builtinCSS = """
    <style>
    @page { size: A4; margin: 18mm 16mm; }
    html { color: #1a1a1a; background: #fff; }
    body { margin: 0 auto; max-width: 46em; font-family: "Helvetica Neue", Helvetica, Arial, sans-serif;
           font-size: 10.5pt; line-height: 1.55; hyphens: auto; overflow-wrap: break-word; }
    h1, h2, h3, h4 { line-height: 1.25; margin: 1.3em 0 .5em; page-break-after: avoid; }
    h1 { font-size: 1.7em; margin-top: 0; } h2 { font-size: 1.3em; } h3 { font-size: 1.1em; }
    a { color: #0969da; text-decoration: none; }
    code { font-family: Menlo, "SF Mono", monospace; font-size: .88em; background: #f0f1f3;
           padding: .1em .3em; border-radius: 3px; }
    pre { background: #f6f8fa; border: 1px solid #d0d7de; border-radius: 5px; padding: 8px 10px;
          white-space: pre-wrap; line-height: 1.4; page-break-inside: avoid; }
    pre code { background: none; padding: 0; }
    blockquote { margin: 0 0 .9em; padding-left: 12px; border-left: 3px solid #c8c8c8; color: #555; }
    table { border-collapse: collapse; margin: .4em 0 1em; }
    th, td { border: 1px solid #bfbfbf; padding: 4px 10px; vertical-align: top; }
    th { background: #f2f2f2; text-align: left; }
    img { max-width: 100%; }
    div.note, div.tip, div.important, div.warning, div.caution { margin: .8em 0; padding: .5em 1em .2em;
          border-left: 3px solid var(--a); background: var(--bg); border-radius: 4px; }
    div.note { --a: #0969da; --bg: #eef5fd; } div.tip { --a: #1a7f37; --bg: #eef8f0; }
    div.important { --a: #8250df; --bg: #f4effc; } div.warning { --a: #9a6700; --bg: #fdf6e6; }
    div.caution { --a: #cf222e; --bg: #fdeff0; }
    div.note > .title p, div.tip > .title p, div.important > .title p, div.warning > .title p,
    div.caution > .title p { margin: 0 0 .3em; font-weight: bold; color: var(--a); }
    </style>
    """
}
