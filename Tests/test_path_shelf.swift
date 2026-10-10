// sources: PythonHelper.swift PathShelf.swift RecentFiles.swift ProcessRun.swift
import AppKit

@main
struct PathShelfTests {
    static var passed = 0
    static var failed = 0

    static func check(_ condition: Bool, _ message: String, line: Int = #line) {
        if condition {
            passed += 1
        } else {
            failed += 1
            print("  FAIL: \(message) (test_path_shelf.swift:\(line))")
        }
    }

    static let fm = FileManager.default

    static func write(_ path: String, _ text: String = "x") {
        try? fm.createDirectory(atPath: (path as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        try? text.write(toFile: path, atomically: true, encoding: .utf8)
    }

    static func main() {
        let tmp = NSTemporaryDirectory() + "pathshelf-\(getpid())"
        try? fm.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        let root = PathShelf.canonical(tmp)?.path ?? tmp
        defer { try? fm.removeItem(atPath: root) }

        PythonHelper.shared.configure(libDir: URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("pylib").path)
        shelf(root)
        clipboard(root)

        print("\npath shelf: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }

    static func shelf(_ root: String) {
        let store = root + "/paths.json"
        let dir = root + "/files"
        let rules = IgnoreRules(home: root, shelfFile: nil)
        rules.gitExcludes = root + "/no-such-global"
        rules.recheck = 0
        let s = PathShelf(store: store, rules: rules)
        s.immediate = true
        s.configure(limit: 99, ignoreFile: nil)
        check(s.limit == 25, "limit clamps to 25 (got \(s.limit))")
        var files: [String] = []
        for i in 0..<30 {
            let f = dir + "/f\(i).txt"
            write(f)
            files.append(f)
            s.add([f], why: .clipboard)
        }
        s.sync()
        var e = s.entries()
        check(e.count == 25, "cap 25 (got \(e.count))")
        check(e.first?.path == files[29] && e.last?.path == files[5], "newest first; the oldest five fell off")
        s.add([files[10]], why: .filefast)
        s.sync()
        e = s.entries()
        check(e.first?.path == files[10] && e.first?.why == .filefast, "a re-add moves to the top, says why")
        check(Set(e.map(\.path)).count == e.count, "no duplicates")
        s.observe(files[10], created: false, origin: nil)
        s.sync()
        check(s.entries().first?.why == .filefast, "an edit keeps 'filefast'")
        write(dir + "/junk.pyc")
        try? "*.pyc\n".write(toFile: root + "/shelf.ignore", atomically: true, encoding: .utf8)
        rules.shelfFile = root + "/shelf.ignore"
        s.observe(dir + "/junk.pyc", created: true, origin: nil)
        s.observe(dir, created: true, origin: nil)
        s.add([dir + "/nope.txt"], why: .clipboard)
        s.sync()
        e = s.entries()
        check(!e.contains { $0.path.hasSuffix("junk.pyc") }, "ignored activity is dropped")
        check(!e.contains { $0.path == dir }, "folders from activity are dropped")
        check(!e.contains { $0.path.hasSuffix("nope.txt") }, "a missing path is never added")
        s.add([dir + "/junk.pyc"], why: .clipboard)
        s.sync()
        check(s.entries().first?.path == dir + "/junk.pyc", "a COPIED path skips the ignore rules")
        let dl = dir + "/report.pdf"
        write(dl)
        s.observe(dl, created: true, origin: "Safari · example.com")
        s.sync()
        check(s.entries().first?.why == .downloaded, "activity with an origin = downloaded")
        let before = s.entries().map(\.path)
        let moved = dir + "/renamed.pdf"
        try? fm.moveItem(atPath: dl, toPath: moved)
        s.renamed(from: dl, to: moved)
        s.sync()
        check(s.entries().map(\.path) == before.map { $0 == dl ? moved : $0 }, "rename re-keys in place")
        try? fm.removeItem(atPath: files[29])
        check(!s.entries().contains { $0.path == files[29] }, "deleted files drop out")
        s.remove([files[28]])
        s.sync()
        check(!s.entries().contains { $0.path == files[28] }, "remove = forget")
        let reloaded = PathShelf(store: store, rules: rules)
        reloaded.configure(limit: 25, ignoreFile: root + "/shelf.ignore")
        check(reloaded.entries().map(\.path) == s.entries().map(\.path), "paths.json round trip")
        reloaded.configure(limit: 3, ignoreFile: root + "/shelf.ignore")
        check(reloaded.entries().count == 3, "a smaller limit trims")
        let fresh = PathShelf(store: root + "/fresh.json", rules: rules)
        fresh.immediate = true
        fresh.configure(limit: 25, ignoreFile: root + "/shelf.ignore")
        fresh.seed(from: [(dir + "/junk.pyc", Date(), nil), (dir, Date(), nil), (files[1], Date(), "AirDrop")])
        fresh.sync()
        check(fresh.entries().map(\.path) == [files[1]] && fresh.entries().first?.why == .downloaded,
              "seed: rules + files only (\(fresh.entries().map(\.path)))")
        check(PathShelf.normalize("/tmp/a/../b") == "/private/tmp/b", "/tmp normalized to /private/tmp")
        let link = root + "/link"
        try? fm.createSymbolicLink(atPath: link, withDestinationPath: dir)
        s.observe(link + "/f3.txt", created: false, origin: nil)
        s.add([link + "/f4.txt"], why: .clipboard)
        s.sync()
        let paths = s.entries().map(\.path)
        check(paths.filter { $0.hasSuffix("/f3.txt") || $0.hasSuffix("/f4.txt") } == [dir + "/f4.txt", dir + "/f3.txt"],
              "a symlinked folder resolves to the real path (\(paths.prefix(3)))")
        check(!paths.contains { $0.hasPrefix(link) }, "no row under the symlink")
        let sock = root + "/s.sock"
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &addr.sun_path) { b in for (k, c) in sock.utf8.enumerated() where k < 103 { b[k] = c } }
        _ = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        s.observe(sock, created: true, origin: nil)
        s.sync()
        check(fm.fileExists(atPath: sock) && !s.entries().contains { $0.path == sock }, "a socket is not a file")
        close(fd)
        check(PathShelf.normalize("file:///Users/x/a%20b.txt") == "/Users/x/a b.txt", "file:// URL normalized")
    }

    static func clipboard(_ root: String) {
        let a = root + "/clip/a file.txt", b = root + "/clip/b.txt"
        write(a)
        write(b)
        let T = ClipboardPaths.paths(inText:)
        check(T(a) == [a], "one path")
        check(T("  \(a)  \n") == [a], "whitespace / trailing newline")
        check(T("'\(a)'") == [a] && T("\"\(a)\"") == [a], "quoted")
        check(T(a.replacingOccurrences(of: " ", with: "\\ ")) == [a], "shell-escaped spaces")
        check(T(URL(fileURLWithPath: a).absoluteString) == [a], "file:// URL")
        check(T("\(a)\n\(b)") == [a, b], "two lines")
        check(T(Array(repeating: b, count: 6).joined(separator: "\n")).isEmpty, "more than 5 lines = not a path list")
        check(T("see \(a) for details").isEmpty, "prose containing a path is ignored")
        check(T("\(a)\nhello").isEmpty, "one non-path line rejects it all")
        check(T(root + "/clip/missing.txt").isEmpty, "missing path ignored")
        check(T("relative/path.txt").isEmpty, "relative path ignored")
        check(T(String(repeating: "x", count: 5000)).isEmpty, "huge text ignored")
        let home = NSHomeDirectory()
        if fm.fileExists(atPath: home + "/.zshrc") || fm.fileExists(atPath: home + "/.bashrc") {
            let f = fm.fileExists(atPath: home + "/.zshrc") ? "~/.zshrc" : "~/.bashrc"
            check(T(f) == [PathShelf.canonical((f as NSString).expandingTildeInPath)?.path ?? "?"],
                  "~/ expanded (and resolved, if it's a link)")
        }

        let pb = NSPasteboard(name: .init("ws-path-shelf-test-\(getpid())"))
        defer { pb.releaseGlobally() }
        pb.clearContents()
        pb.writeObjects([URL(fileURLWithPath: a) as NSURL, URL(fileURLWithPath: b) as NSURL])
        check(ClipboardPaths.paths(in: pb) == [a, b], "file URLs (a Finder copy)")
        pb.clearContents()
        pb.setString(a, forType: .string)
        check(ClipboardPaths.paths(in: pb) == [a], "text path")
        pb.clearContents()
        pb.declareTypes([.string, .init("org.nspasteboard.ConcealedType")], owner: nil)
        pb.setString(a, forType: .string)
        check(ClipboardPaths.paths(in: pb).isEmpty, "password-manager (concealed) copies skipped")

        let w = ClipboardPaths(pasteboard: pb)
        var got: [[String]] = []
        w.onPaths = { got.append($0) }
        pb.clearContents()
        pb.setString(b, forType: .string)
        w.check()
        w.check()
        check(got == [[b]], "one change → one report (\(got))")
        pb.clearContents()
        pb.setString(a, forType: .string)
        w.ownWrite()
        w.check()
        check(got.count == 1, "our own copy is not reported")
        pb.clearContents()
        pb.setString("not a path", forType: .string)
        w.check()
        check(got.count == 1, "junk is not reported")
    }
}
