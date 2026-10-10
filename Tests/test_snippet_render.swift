// sources: ProsePDF.swift ProcessRun.swift
import Foundation

@main
struct Main {
    static func main() {
        let fm = FileManager.default
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path
        let here = root + "/Tests/snippet_render"
        let nvim = ["/opt/homebrew/bin/nvim", "/usr/local/bin/nvim"].first { fm.isExecutableFile(atPath: $0) }
        var c = ProsePDF.Config()
        let diagrams = ProsePDF.expand("~/.dotfiles/markdown_generator/diagrams.lua")
        c.filter = fm.fileExists(atPath: diagrams) ? diagrams : "none"
        let wp = (try? fm.destinationOfSymbolicLink(atPath: c.engine)).map {
            (( c.engine as NSString).deletingLastPathComponent as NSString).appendingPathComponent($0)
        } ?? c.engine
        let python = (try? String(contentsOfFile: wp, encoding: .utf8))?
            .split(separator: "\n").first.map { String($0.dropFirst(2)) } ?? ""
        guard let nvim, fm.isExecutableFile(atPath: c.pandoc), fm.isExecutableFile(atPath: python) else {
            print("  (nvim / pandoc / weasyprint missing: snippet render skipped)"); exit(0)
        }

        let tmp = NSTemporaryDirectory() + "snippet-render-\(getpid())"
        try? fm.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(atPath: tmp) }
        c.cacheDir = tmp + "/cache"
        setenv("WS_REPO", root, 1)
        setenv("OUT", tmp, 1)
        guard let ex = try? runProcess(nvim, ["--headless", "--clean", "-u", "NONE", "-c", "luafile \(here)/expand.lua", "-c", "qa!"]) else {
            print("  FAIL: nvim did not run"); exit(1)
        }
        if ex.err.contains("EXPAND FAIL") { print("  FAIL: " + ex.err); exit(1) }

        let notes = ((try? fm.contentsOfDirectory(atPath: tmp)) ?? []).filter { $0.hasSuffix(".md") }.sorted()
        let css = tmp + "/header.css"
        try? Data(ProsePDF.builtinCSS.utf8).write(to: URL(fileURLWithPath: css))
        let t0 = Date()
        var failed = 0
        let lock = NSLock()
        DispatchQueue.concurrentPerform(iterations: notes.count) { i in
            let note = tmp + "/" + notes[i], stem = (note as NSString).deletingPathExtension
            for (sp, ext) in [(false, "pdf"), (true, "scr")] {
                let p = try? runProcess(c.pandoc, ProsePDF.pandocArgs(note: note, css: css, html: "\(stem).\(ext).html", c, sourcepos: sp))
                if p?.code != 0 { print("  FAIL: pandoc (\(ext)) on \(notes[i]): \(p?.err ?? "did not run")"); lock.lock(); failed += 1; lock.unlock() }
            }
        }
        print("  rendered \(notes.count) snippets × 2 in \(Int(Date().timeIntervalSince(t0) * 1000)) ms")
        let cmp = try? runProcess(python, ["-I", here + "/dom_compare.py", tmp])
        print((cmp?.out ?? "") + (cmp?.err ?? ""), terminator: "")
        exit(failed == 0 && cmp?.code == 0 ? 0 : 1)
    }
}
