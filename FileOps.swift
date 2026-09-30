import Foundation

// The file browser's file operations (trash / new / duplicate / copy / move)
// and their undo. Synchronous and AppKit-free: callers run them off the main
// thread and report `Outcome.changes` to the Recent list themselves
// (`FileDrag.onFileOp`). Every op that succeeds pushes ONE undo record.
// Tests: bin/run-tests.sh fileops (Tests/test_file_ops.swift).
enum FileOps {
    // what was done: a file that moved (from → to) or appeared (from nil)
    typealias Change = (from: String?, to: String)

    struct Outcome {
        var changes: [Change] = []
        var failed: String?          // the first error, "name: why"
        var paths: [String] { changes.map { $0.to } }
    }

    enum Record {
        case moved([(from: String, to: String)])     // rename / move: move back
        case created([String])                       // copy / duplicate / new: trash it
        case trashed([(from: String, to: String)])   // trash: put back
    }

    private static let lock = NSLock()
    private static var stack: [Record] = []
    private static let limit = 50

    static var canUndo: Bool {
        lock.lock(); defer { lock.unlock() }
        return !stack.isEmpty
    }

    static func push(_ r: Record) {
        lock.lock(); defer { lock.unlock() }
        stack.append(r)
        if stack.count > limit { stack.removeFirst(stack.count - limit) }
    }

    private static func pop() -> Record? {
        lock.lock(); defer { lock.unlock() }
        return stack.popLast()
    }

    static func forgetUndo() {
        lock.lock(); defer { lock.unlock() }
        stack.removeAll()
    }

    // "name.ext" -> "name 2.ext", "name 3.ext"… until it's free
    static func freeURL(_ name: String, in dir: URL) -> URL {
        let fm = FileManager.default
        var url = dir.appendingPathComponent(name)
        let base = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        var n = 2
        while fm.fileExists(atPath: url.path) {
            url = dir.appendingPathComponent(ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)")
            n += 1
        }
        return url
    }

    // Finder's duplicate name: "name copy.ext", then "name copy 2.ext"…
    static func copyURL(of url: URL) -> URL {
        let name = url.lastPathComponent
        let base = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        let stem = base.isEmpty ? name : base
        let copy = ext.isEmpty || base.isEmpty ? "\(stem) copy" : "\(stem) copy.\(ext)"
        return freeURL(copy, in: url.deletingLastPathComponent())
    }

    private static func note(_ out: inout Outcome, _ name: String, _ error: Error) {
        if out.failed == nil { out.failed = "\(name): \(error.localizedDescription)" }
    }

    // copy / move `urls` into `dir`; name clashes keep both
    static func transfer(_ urls: [URL], into dir: String, move: Bool) -> Outcome {
        let fm = FileManager.default
        let dest = URL(fileURLWithPath: dir, isDirectory: true)
        var out = Outcome()
        for u in urls {
            let src = u.standardizedFileURL
            // a folder into itself / its own subfolder
            if dir == src.path || dir.hasPrefix(src.path + "/") {
                if out.failed == nil { out.failed = "\(u.lastPathComponent): can't go inside itself" }
                continue
            }
            // moving to where it already is: nothing to do
            if move, src.deletingLastPathComponent().path == dest.standardizedFileURL.path { continue }
            let target = freeURL(u.lastPathComponent, in: dest)
            do {
                if move { try fm.moveItem(at: src, to: target) } else { try fm.copyItem(at: src, to: target) }
                out.changes.append((move ? src.path : nil, target.path))
            } catch {
                note(&out, u.lastPathComponent, error)
            }
        }
        if !out.changes.isEmpty {
            push(move ? .moved(out.changes.map { ($0.from ?? "", $0.to) }) : .created(out.paths))
        }
        return out
    }

    static func duplicate(_ paths: [String]) -> Outcome {
        var out = Outcome()
        for p in paths {
            let src = URL(fileURLWithPath: p)
            let target = copyURL(of: src)
            do {
                try FileManager.default.copyItem(at: src, to: target)
                out.changes.append((nil, target.path))
            } catch {
                note(&out, src.lastPathComponent, error)
            }
        }
        if !out.changes.isEmpty { push(.created(out.paths)) }
        return out
    }

    private static func trashOnly(_ paths: [String]) -> Outcome {
        var out = Outcome()
        for p in paths {
            var landed: NSURL?
            do {
                try FileManager.default.trashItem(at: URL(fileURLWithPath: p), resultingItemURL: &landed)
                out.changes.append((p, landed?.path ?? p))
            } catch {
                note(&out, (p as NSString).lastPathComponent, error)
            }
        }
        return out
    }

    // move to the Trash; `changes` = (where it was, where it is in the Trash)
    static func trash(_ paths: [String]) -> Outcome {
        let out = trashOnly(paths)
        if !out.changes.isEmpty { push(.trashed(out.changes.map { ($0.from ?? "", $0.to) })) }
        return out
    }

    // an empty folder / file named `name` in `dir` (kept free: "untitled folder 2")
    static func create(_ name: String, in dir: String, folder: Bool) -> Outcome {
        let fm = FileManager.default
        let url = freeURL(name, in: URL(fileURLWithPath: dir, isDirectory: true))
        var out = Outcome()
        do {
            if folder {
                try fm.createDirectory(at: url, withIntermediateDirectories: false)
            } else {
                try Data().write(to: url, options: .withoutOverwriting)
            }
            out.changes.append((nil, url.path))
            push(.created([url.path]))
        } catch {
            note(&out, url.lastPathComponent, error)
        }
        return out
    }

    // a rename the caller did itself
    static func recordRename(from: String, to: String) {
        push(.moved([(from, to)]))
    }

    private static func moveBack(_ pairs: [(from: String, to: String)]) -> Outcome {
        let fm = FileManager.default
        var out = Outcome()
        for (from, to) in pairs.reversed() {
            // a case-only rename: the "old" name still exists — as this file
            let sameFile = from.lowercased() == to.lowercased()
            if fm.fileExists(atPath: from), !sameFile {
                if out.failed == nil {
                    out.failed = "\((from as NSString).lastPathComponent): something else is there now"
                }
                continue
            }
            do {
                try fm.moveItem(atPath: to, toPath: from)
                out.changes.append((to, from))
            } catch {
                note(&out, (to as NSString).lastPathComponent, error)
            }
        }
        return out
    }

    // undo the last op; nil = nothing to undo. `what` reads "undid <what>".
    static func undo() -> (what: String, outcome: Outcome)? {
        guard let r = pop() else { return nil }
        func n(_ c: Int, _ one: String) -> String { c == 1 ? one : "\(c) items" }
        switch r {
        case .moved(let pairs):
            let one = pairs.first.map { p -> String in
                let same = (p.from as NSString).deletingLastPathComponent == (p.to as NSString).deletingLastPathComponent
                return (same ? "rename of " : "move of ") + (p.from as NSString).lastPathComponent
            } ?? "move"
            return (pairs.count == 1 ? one : "move of \(pairs.count) items", moveBack(pairs))
        case .created(let paths):
            let live = paths.filter { FileManager.default.fileExists(atPath: $0) }
            return ("creating " + n(paths.count, (paths.first.map { ($0 as NSString).lastPathComponent } ?? "")),
                    trashOnly(live))
        case .trashed(let pairs):
            return ("trash of " + n(pairs.count, (pairs.first.map { ($0.from as NSString).lastPathComponent } ?? "")),
                    moveBack(pairs))
        }
    }
}
