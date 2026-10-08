// sources: AIFormat.swift ProsePDF.swift ProcessRun.swift
// Prose: highlighted fences (RichText.pandocHTML highlight:) and Export PDF
// (ProsePDF: pandoc -s → weasyprint). The real run needs both installed.
// Usage: bin/run-tests.sh prose

import Foundation

func aiSetting(_ key: String, _ fallback: String) -> String { fallback }
func aiNumber(_ key: String, _ fallback: CGFloat) -> CGFloat { fallback }

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition { passed += 1 } else { failed += 1; print("  FAIL: \(message) (test_prose_pdf.swift:\(line))") }
}

@main
struct Main {
    static func main() {
        let fence = "```cpp\n#include <iostream>\nint main(){ std::cout << \"hello\"; }\n```\n"
        if RichText.available {
            let hl = RichText.pandocHTML(fence, highlight: true) ?? ""
            check(hl.contains("sourceCode cpp"), "prose fence is a sourceCode block: \(hl)")
            check(hl.contains("class=\"dt\""), "int is a type token: \(hl)")
            let plain = RichText.pandocHTML(fence) ?? ""
            check(plain.contains("<pre class=\"cpp\"><code>") && !plain.contains("<span"), "pastes stay unhighlighted: \(plain)")
        } else { print("  (pandoc missing: highlight checks skipped)") }

        var c = ProsePDF.Config()
        c.outDir = "~/Out"
        check(ProsePDF.outputPath(note: "/n/My Note.md", c) == NSHomeDirectory() + "/Out/My Note.pdf", "output = stem.pdf in pdf-path")
        let pa = ProsePDF.pandocArgs(note: "/n/a.md", css: "/c.html", html: "/t/pdf.html", c)
        check(pa.contains("--syntax-highlighting=tango") && pa.contains("--include-in-header=/c.html")
              && pa.contains("--resource-path=/n") && pa.last == "/n/a.md", "pandoc argv: \(pa)")
        // diagram filter: beside pdf-css by default, "none" turns it off, a missing file is skipped
        let ftmp = NSTemporaryDirectory() + "ws-filter-test"
        try? FileManager.default.createDirectory(atPath: ftmp, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: ftmp + "/diagrams.lua", contents: Data("return {}".utf8))
        var fc = c
        fc.css = ftmp + "/style.css"
        check(ProsePDF.pandocArgs(note: "/n/a.md", css: "/c.html", html: "/t/pdf.html", fc).contains("--lua-filter=\(ftmp)/diagrams.lua"),
              "diagrams.lua beside pdf-css is used")
        fc.filter = "none"
        check(!ProsePDF.pandocArgs(note: "/n/a.md", css: "/c.html", html: "/t/pdf.html", fc).contains { $0.hasPrefix("--lua-filter") },
              "pdf-filter = none turns it off")
        fc.filter = "/nope/missing.lua"
        check(ProsePDF.filterPaths(fc).isEmpty, "a missing filter file is skipped")
        check(ProsePDF.engineArgs(note: "/n/a.md", html: "/t/p.html", out: "/o.pdf") == ["--pdf-tags", "-u", "file:///n/", "/t/p.html", "/o.pdf"],
              "weasyprint argv")

        // missing engine → a hint, no crash
        var bad = c
        bad.engine = "/nonexistent/weasyprint"
        if case .failure(let e) = ProsePDF.export(note: "/n/a.md", bad) {
            check(e.description.contains("weasyprint"), "missing engine names it: \(e)")
        } else { check(false, "missing engine must fail") }

        // the real thing
        let tmp = NSTemporaryDirectory() + "prose-pdf-\(getpid())"
        try? FileManager.default.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: tmp) }
        var real = ProsePDF.Config()
        real.outDir = tmp + "/out"
        real.cacheDir = tmp + "/cache"
        if FileManager.default.isExecutableFile(atPath: real.pandoc), FileManager.default.isExecutableFile(atPath: real.engine) {
            let note = tmp + "/demo note.md"
            try? "# Demo\n\n> [!NOTE]\n> hi\n\n\(fence)".write(toFile: note, atomically: true, encoding: .utf8)
            let t0 = Date()
            switch ProsePDF.export(note: note, real) {
            case .success(let out):
                check(out == tmp + "/out/demo note.pdf", "pdf path: \(out)")
                let head = (try? FileHandle(forReadingFrom: URL(fileURLWithPath: out)).readData(ofLength: 5)) ?? Data()
                check(String(data: head, encoding: .ascii) == "%PDF-", "is a PDF")
                print("  export: \(Int(Date().timeIntervalSince(t0) * 1000)) ms")
            case .failure(let e): check(false, "export failed: \(e)")
            }
        } else { print("  (pandoc / weasyprint missing: real export skipped)") }

        print("prose: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
