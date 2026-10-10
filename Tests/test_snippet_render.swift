// sources: PythonHelper.swift ProcessRun.swift
import Foundation

@main
struct Main {
    static func main() {
        let fm = FileManager.default
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path
        let here = root + "/Tests/snippet_render"
        let nvim = ["/opt/homebrew/bin/nvim", "/usr/local/bin/nvim"].first { fm.isExecutableFile(atPath: $0) }
        let pandoc = "/opt/homebrew/bin/pandoc"
        let engine = "/opt/homebrew/bin/weasyprint"
        let helper = PythonHelper(libDir: root + "/pylib")
        var config: [String: Any] = ["pandoc": pandoc, "engine": engine]
        let diagrams = ("~/.dotfiles/markdown_generator/diagrams.lua" as NSString).expandingTildeInPath
        config["filter"] = fm.fileExists(atPath: diagrams) ? diagrams : "none"
        let wp = (try? fm.destinationOfSymbolicLink(atPath: engine)).map {
            ((engine as NSString).deletingLastPathComponent as NSString).appendingPathComponent($0)
        } ?? engine
        let python = (try? String(contentsOfFile: wp, encoding: .utf8))?
            .split(separator: "\n").first.map { String($0.dropFirst(2)) } ?? ""
        guard let nvim, fm.isExecutableFile(atPath: pandoc), fm.isExecutableFile(atPath: python) else {
            print("  (nvim / pandoc / weasyprint missing: snippet render skipped)"); exit(0)
        }

        let tmp = NSTemporaryDirectory() + "snippet-render-\(getpid())"
        try? fm.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        defer { try? fm.removeItem(atPath: tmp) }
        config["cacheDir"] = tmp + "/cache"
        guard case .success(let cssBox) = helper.callSync("prose.css_content", ["config": config]),
              let css = (cssBox as? [String: Any])?["css"] as? String else {
            print("  FAIL: python helper did not answer css_content"); exit(1)
        }
        setenv("WS_REPO", root, 1)
        setenv("OUT", tmp, 1)
        guard let ex = try? runProcess(nvim, ["--headless", "--clean", "-u", "NONE", "-c", "luafile \(here)/expand.lua", "-c", "qa!"]) else {
            print("  FAIL: nvim did not run"); exit(1)
        }
        if ex.err.contains("EXPAND FAIL") { print("  FAIL: " + ex.err); exit(1) }

        let notes = ((try? fm.contentsOfDirectory(atPath: tmp)) ?? []).filter { $0.hasSuffix(".md") }.sorted()
        let cssPath = tmp + "/header.css"
        try? Data(css.utf8).write(to: URL(fileURLWithPath: cssPath))
        let cfg = config
        let t0 = Date()
        var failed = 0
        let lock = NSLock()
        DispatchQueue.concurrentPerform(iterations: notes.count) { i in
            let note = tmp + "/" + notes[i], stem = (note as NSString).deletingPathExtension
            for (sp, ext) in [(false, "pdf"), (true, "scr")] {
                guard case .success(let box) = helper.callSync("prose.pandoc_args",
                        ["note": note, "css": cssPath, "html": "\(stem).\(ext).html",
                         "config": cfg, "sourcepos": sp]),
                      let args = (box as? [String: Any])?["args"] as? [String] else {
                    print("  FAIL: python helper did not answer pandoc_args for \(notes[i])")
                    lock.lock(); failed += 1; lock.unlock()
                    continue
                }
                let p = try? runProcess(pandoc, args)
                if p?.code != 0 { print("  FAIL: pandoc (\(ext)) on \(notes[i]): \(p?.err ?? "did not run")"); lock.lock(); failed += 1; lock.unlock() }
            }
        }
        print("  rendered \(notes.count) snippets × 2 in \(Int(Date().timeIntervalSince(t0) * 1000)) ms")
        let cmp = try? runProcess(python, ["-I", here + "/dom_compare.py", tmp])
        print((cmp?.out ?? "") + (cmp?.err ?? ""), terminator: "")
        exit(failed == 0 && cmp?.code == 0 ? 0 : 1)
    }
}
