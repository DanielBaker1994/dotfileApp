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
        var cacheDir = NSHomeDirectory() + "/.cache/kitchen-sink/prose"
    }

    static func expand(_ p: String) -> String { (p as NSString).expandingTildeInPath }

    static func outputPath(note: String, _ c: Config) -> String {
        let stem = ((note as NSString).lastPathComponent as NSString).deletingPathExtension
        return (expand(c.outDir) as NSString).appendingPathComponent((stem.isEmpty ? "note" : stem) + ".pdf")
    }

    static func pandocArgs(note: String, css: String, html: String, _ c: Config) -> [String] {
        let dir = (note as NSString).deletingLastPathComponent
        let stem = ((note as NSString).lastPathComponent as NSString).deletingPathExtension
        return ["-s", "-f", "gfm", "-t", "html5", "--syntax-highlighting=\(c.highlight.isEmpty ? "tango" : c.highlight)",
                "-V", "lang=en", "--metadata", "pagetitle=\(stem)", "--resource-path=\(dir)",
                "--include-in-header=\(css)", "-o", html, note]
    }

    static func engineArgs(note: String, html: String, out: String) -> [String] {
        let base = URL(fileURLWithPath: (note as NSString).deletingLastPathComponent, isDirectory: true).absoluteString
        return ["-u", base, html, out]
    }

    // the header CSS file: the configured one, else the built-in style
    static func headerFile(_ c: Config) throws -> String {
        let own = expand(c.css)
        if !c.css.isEmpty, FileManager.default.fileExists(atPath: own) { return own }
        let f = (c.cacheDir as NSString).appendingPathComponent("pdf-header.html")
        try Data(builtinCSS.utf8).write(to: URL(fileURLWithPath: f))
        return f
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
            let html = (c.cacheDir as NSString).appendingPathComponent("pdf.html")
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
