// sources: PythonHelper.swift CompareText.swift CompareFolder.swift FileOps.swift
import Foundation

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_compare_folder.swift:\(line))")
    }
}

@main
struct FolderTests {
    static let fm = FileManager.default

    static func write(_ p: String, _ s: String, mtime: Double? = nil) {
        try? fm.createDirectory(atPath: (p as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        fm.createFile(atPath: p, contents: Data(s.utf8))
        if let m = mtime { try? fm.setAttributes([.modificationDate: Date(timeIntervalSince1970: m)], ofItemAtPath: p) }
    }

    static func node(_ t: FolderTree, _ rel: String) -> FolderNode? { t.all.first { $0.rel == rel } }

    static func syncAndLinks(_ tmp: String) {
        let A = tmp + "/sync/A", B = tmp + "/sync/B"
        let t0 = 1_700_000_000.0
        write(A + "/same.txt", "s\n", mtime: t0)
        write(B + "/same.txt", "s\n", mtime: t0)
        write(A + "/lnew.txt", "left new\n", mtime: t0 + 100)
        write(B + "/lnew.txt", "old\n", mtime: t0)
        write(A + "/rnew.txt", "old\n", mtime: t0)
        write(B + "/rnew.txt", "right new\n", mtime: t0 + 100)
        write(A + "/tie.txt", "one\n", mtime: t0)
        write(B + "/tie.txt", "two!\n", mtime: t0)
        write(A + "/aonly/x.txt", "x\n")
        write(B + "/bonly.txt", "b\n")
        var o = FolderOptions()
        o.content = "always"
        var t = FolderScan.run(left: A, right: B, options: o)
        let pend = t.pending
        for n in pend { FolderContent.apply(FolderContent.check(left: t.path(n, .left), right: t.path(n, .right), size: (n.left!.size, n.right!.size), imp: o.importance), to: n) }
        t.settle()
        let ur = SyncPlan.make(t, .updateRight)
        check(Set(ur.copies.map(\.rel)) == ["lnew.txt", "aonly"] && ur.trash.isEmpty, "update right: newer + left-only (\(ur.copies.map(\.rel)))")
        check(ur.skipped == ["tie.txt"], "update right: a tie is skipped")
        let ub = SyncPlan.make(t, .updateBoth)
        check(Set(ub.copies.map(\.rel)) == ["lnew.txt", "aonly", "rnew.txt", "bonly.txt"], "update both")
        let mr = SyncPlan.make(t, .mirrorRight)
        check(Set(mr.copies.map(\.rel)) == ["lnew.txt", "rnew.txt", "tie.txt", "aonly"], "mirror right copies every difference")
        check(mr.trash.map(\.rel) == ["bonly.txt"] && mr.trash[0].side == .right, "mirror right trashes right-only")
        check(SyncPlan.make(t, .mirrorRight, nameFilter: "*.md").isEmpty, "a name filter narrows the plan")

        let stack = FileOps.UndoStack()
        FileOps.forgetUndo()
        let mark = stack.count
        _ = FileOps.place(mr.copies.map { ($0.src, $0.dst) }, move: false, clash: .replace, undo: stack)
        _ = FileOps.trash(mr.trash.map(\.path), undo: stack)
        stack.collapse(since: mark, "mirror")
        check(stack.count == 1 && !FileOps.canUndo, "sync = one step on the session's stack, none on the shared one")
        t = FolderScan.run(left: A, right: B, options: o)
        for n in t.pending { FolderContent.apply(FolderContent.check(left: t.path(n, .left), right: t.path(n, .right), size: (n.left!.size, n.right!.size), imp: o.importance), to: n) }
        t.settle()
        let c = t.counts()
        check(c.different + c.leftOnly + c.rightOnly == 0, "mirrored: identical (\(c))")
        let u = FileOps.undo(stack)
        check(u?.what == "mirror", "undo the sync as one step")
        check((try? String(contentsOfFile: B + "/lnew.txt", encoding: .utf8)) == "old\n" && fm.fileExists(atPath: B + "/bonly.txt")
              && !fm.fileExists(atPath: B + "/aonly"), "undo restores the right side")
        check(!stack.canUndo, "the session stack is empty again")

        let G = tmp + "/git/L", H = tmp + "/git/R", W = tmp + "/git/work"
        write(G + "/a.txt", "same\n", mtime: t0)
        write(W + "/a.txt", "same\n", mtime: t0 + 999)
        write(G + "/b.txt", "old\n", mtime: t0)
        write(W + "/b.txt", "new!\n", mtime: t0 + 999)
        try? fm.createDirectory(atPath: H, withIntermediateDirectories: true)
        try? fm.createSymbolicLink(atPath: H + "/a.txt", withDestinationPath: W + "/a.txt")
        try? fm.createSymbolicLink(atPath: H + "/b.txt", withDestinationPath: W + "/b.txt")
        var go = FolderOptions()
        go.content = "auto"
        let gt = FolderScan.run(left: G, right: H, options: go)
        let done = DispatchSemaphore(value: 0)
        FolderContent.run(tree: gt, nodes: gt.pending, imp: go.importance, queue: DispatchQueue(label: "t"), cancelled: { false },
                          batch: { for (id, a) in $0 { FolderContent.apply(a, to: gt.node(id)!) } }, done: { done.signal() })
        done.wait()
        check(node(gt, "a.txt")?.status == .same, "a link to an identical file = same (\(node(gt, "a.txt")?.status.rawValue ?? "-"))")
        check(node(gt, "b.txt")?.status == .different && node(gt, "b.txt")?.newer == .right, "a link to a changed file differs, newer right")
    }

    static func main() {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path
        PythonHelper.shared.configure(libDir: root + "/pylib")
        let tmp = NSTemporaryDirectory() + "compare-folder-test-\(getpid())"
        try? fm.removeItem(atPath: tmp)
        let L = tmp + "/L", R = tmp + "/R"
        try? fm.createDirectory(atPath: L, withIntermediateDirectories: true)
        try? fm.createDirectory(atPath: R, withIntermediateDirectories: true)
        defer { try? fm.removeItem(atPath: tmp) }

        let t0 = 1_700_000_000.0
        write(L + "/same.txt", "a\nb\n", mtime: t0)
        write(R + "/same.txt", "a\nb\n", mtime: t0 + 1)
        write(L + "/size.txt", "short\n", mtime: t0)
        write(R + "/size.txt", "much longer\n", mtime: t0 + 100)
        write(L + "/newer.txt", "xx\n", mtime: t0 + 500)
        write(R + "/newer.txt", "yy\n", mtime: t0)
        write(L + "/touched.txt", "same bytes\n", mtime: t0)
        write(R + "/touched.txt", "same bytes\n", mtime: t0 + 900)
        write(L + "/ws.txt", "a b\n", mtime: t0)
        write(R + "/ws.txt", "a b \n", mtime: t0 + 50)
        write(L + "/only-left.txt", "l\n")
        write(R + "/only-right.txt", "r\n")
        write(L + "/sub/deep/x.txt", "1\n", mtime: t0)
        write(R + "/sub/deep/x.txt", "2\n", mtime: t0 + 300)
        write(L + "/sub/ok.txt", "ok\n", mtime: t0)
        write(R + "/sub/ok.txt", "ok\n", mtime: t0)
        write(L + "/clean/a.txt", "a\n", mtime: t0)
        write(R + "/clean/a.txt", "a\n", mtime: t0)
        write(L + "/orph/inside.txt", "i\n")
        write(L + "/node_modules/pkg/index.js", "x\n")
        write(R + "/node_modules/pkg/index.js", "y\n")
        write(L + "/Mixed.TXT", "a\n", mtime: t0)
        try? fm.createSymbolicLink(atPath: L + "/link", withDestinationPath: "target-a")
        try? fm.createSymbolicLink(atPath: R + "/link", withDestinationPath: "target-b")
        try? fm.createDirectory(atPath: L + "/kind", withIntermediateDirectories: true)
        write(R + "/kind", "a file here, a folder there\n")

        var o = FolderOptions()
        o.exclude = ["node_modules"]
        let tree = FolderScan.run(left: L, right: R, options: o)

        func st(_ rel: String) -> FolderStatus? { node(tree, rel)?.status }
        check(st("same.txt") == .same, "equal size, mtimes inside the tolerance: same")
        check(st("size.txt") == .different && node(tree, "size.txt")?.newer == .right, "size differs: different, right newer")
        check(st("newer.txt") == .unknown, "same size + different time: unknown until the content check")
        check(node(tree, "newer.txt")?.newer == .left, "newer.txt: left newer")
        check(st("touched.txt") == .unknown, "touched.txt waits for content")
        check(st("only-left.txt") == .leftOnly && st("only-right.txt") == .rightOnly, "orphans")
        check(st("orph") == .leftOnly && st("orph/inside.txt") == .leftOnly, "an orphan folder lists its content as orphans")
        check(node(tree, "node_modules") == nil, "exclude: node_modules never scanned")
        check(st("link") == .different, "symlinks compare by target")
        check(st("kind") == .different, "file vs folder = different")
        check(st("clean") == .same, "a clean folder is same")
        check(st("sub") == .unknown, "a folder with unchecked pairs is unknown until the content answers")
        check(tree.pending.count == 3, "pending = unknown pairs (\(tree.pending.count))")

        let pend = tree.pending
        let sem = DispatchSemaphore(value: 0)
        let q = DispatchQueue(label: "t")
        var answers: [Int: FolderContent.Answer] = [:]
        FolderContent.run(tree: tree, nodes: pend, imp: o.importance, queue: q, cancelled: { false },
                          batch: { for (id, a) in $0 { answers[id] = a } }, done: { sem.signal() })
        sem.wait()
        for n in pend { if let a = answers[n.id] { FolderContent.apply(a, to: n) } }
        tree.settle()
        check(st("newer.txt") == .different, "content differs → different")
        check(st("sub") == .different && st("sub/deep") == .different, "a folder is red when something inside differs")
        check(st("touched.txt") == .same, "content same, mtime differs → same")
        check(st("ws.txt") == .unknown || st("ws.txt") == .unimportant || st("ws.txt") == .different, "ws.txt settled")
        let cands = FolderContent.ruleCandidates(tree)
        check(cands.contains { $0.rel == "ws.txt" } || st("ws.txt") == .unimportant, "ws.txt is a rules candidate")
        for n in cands where FolderContent.check(left: tree.path(n, .left), right: tree.path(n, .right),
                                                  size: (n.left!.size, n.right!.size), imp: o.importance) == .unimportant {
            n.status = .unimportant
        }
        tree.settle()
        check(st("ws.txt") == .unimportant, "whitespace-only difference → unimportant (blue)")
        check(st("size.txt") == .different, "size.txt stays different under the rules")

        var never = o; never.content = "never"
        let t2 = FolderScan.run(left: L, right: R, options: never)
        check(t2.pending.isEmpty, "content = never: nothing pending")
        check(node(t2, "newer.txt")?.status == .different, "never: same size + different time = different")
        var always = o; always.content = "always"
        let t3 = FolderScan.run(left: L, right: R, options: always)
        check(node(t3, "same.txt")?.status == .unknown, "always: even equal times are checked")

        let ML = tmp + "/ML", MR = tmp + "/MR"
        write(ML + "/masked.txt", "x\n", mtime: t0)
        write(MR + "/masked.txt", "y\n", mtime: t0)
        write(ML + "/touched.txt", "z\n", mtime: t0)
        write(MR + "/touched.txt", "z\n", mtime: t0 + 500)
        let tm = FolderScan.run(left: ML, right: MR, options: FolderOptions())
        check(node(tm, "masked.txt")?.status == .same && node(tm, "masked.txt")?.sameByMetadata == true,
              "auto: same size + time = same, flagged as metadata-only")
        check(tm.counts().sameByMetadata == 1, "counts: one same by date/size (\(tm.counts().sameByMetadata))")
        let mp = tm.pending
        let msem = DispatchSemaphore(value: 0)
        var mans: [Int: FolderContent.Answer] = [:]
        FolderContent.run(tree: tm, nodes: mp, imp: o.importance, queue: q, cancelled: { false },
                          batch: { for (id, a) in $0 { mans[id] = a } }, done: { msem.signal() })
        msem.wait()
        for n in mp { if let a = mans[n.id] { FolderContent.apply(a, to: n) } }
        tm.settle()
        check(node(tm, "touched.txt")?.status == .same && node(tm, "touched.txt")?.sameByMetadata == false,
              "a same read from the bytes is not flagged")
        let tma = FolderScan.run(left: ML, right: MR, options: always)
        check(node(tma, "masked.txt")?.status == .unknown && node(tma, "masked.txt")?.sameByMetadata == false,
              "always: nothing is same by metadata")
        var mnever = FolderOptions(); mnever.content = "never"
        check(FolderScan.run(left: ML, right: MR, options: mnever).counts().sameByMetadata == 1, "never: metadata-only same is flagged")

        var v = FolderTree.View()
        let all = tree.rows(v)
        check(all.contains { $0.node.rel == "same.txt" }, "All lists same files")
        check(!all.contains { $0.node.rel == "sub/ok.txt" }, "a collapsed folder hides its children")
        v.filter = .diffs
        let diffs = tree.rows(v).map(\.node.rel)
        check(diffs.contains("sub/deep/x.txt") && !diffs.contains("same.txt") && !diffs.contains("clean"), "Differences opens folders, hides same")
        v.filter = .orphans
        let orph = tree.rows(v).map(\.node.rel)
        check(orph.contains("only-left.txt") && orph.contains("only-right.txt") && !orph.contains("size.txt"), "Orphans")
        v.filter = .rightNewer
        check(tree.rows(v).map(\.node.rel).contains("size.txt"), "Right newer")
        v = FolderTree.View()
        v.flatten = true
        v.nameFilter = "*.txt, !same*"
        let flat = tree.rows(v).map(\.node.rel)
        check(flat.contains("sub/deep/x.txt") && !flat.contains("same.txt") && flat.allSatisfy { !$0.hasSuffix("js") }, "flatten + name filter")
        check(FolderTree.matchesName("Foo.SWIFT", "*.swift"), "name filter is case-insensitive")
        check(FolderTree.matchesName("anything", ""), "empty name filter passes")
        let cnt = tree.counts()
        check(cnt.leftOnly >= 2 && cnt.rightOnly == 1, "counts: orphans (\(cnt.leftOnly)/\(cnt.rightOnly))")

        let shelf = tmp + "/test.ignore"
        write(shelf, "sub/\n")
        var ig = FolderOptions()
        ig.useGitignore = true
        ig.ignoreFile = shelf
        let t4 = FolderScan.run(left: L, right: R, options: ig)
        check(node(t4, "sub") == nil && node(t4, "sub/ok.txt") == nil, "ignore rules: an ignored folder is never walked")

        write(R + "/mixed.txt", "a\n", mtime: t0)
        let t5 = FolderScan.run(left: L, right: R, options: o)
        if t5.caseInsensitive {
            check(node(t5, "Mixed.TXT")?.right != nil, "case-insensitive volume: Mixed.TXT pairs with mixed.txt")
        } else {
            check(node(t5, "Mixed.TXT")?.right == nil, "case-sensitive volume: no pairing")
        }
        write(L + "/caf\u{00e9}.txt", "n\n", mtime: t0)
        write(R + "/cafe\u{0301}.txt", "n\n", mtime: t0)
        let t6 = FolderScan.run(left: L, right: R, options: o)
        check(node(t6, "caf\u{00e9}.txt")?.right != nil, "NFC / NFD names pair")

        if let n = node(tree, "only-left.txt") {
            try? fm.copyItem(atPath: L + "/only-left.txt", toPath: R + "/only-left.txt")
            FolderScan.restat(n, tree: tree, o)
            check(n.right != nil && (n.status == .same || n.status == .unknown), "restat sees the copy (\(n.status))")
        }

        let P = tmp + "/P", Q = tmp + "/Q"
        write(P + "/f.txt", "new\n")
        write(Q + "/f.txt", "old\n")
        write(P + "/d/inner.txt", "inner\n")
        var out = FileOps.place([(P + "/f.txt", Q + "/f.txt"), (P + "/d", Q + "/deep/er/d")], move: false, clash: .replace)
        check(out.failed == nil, "place copy: \(out.failed ?? "ok")")
        check((try? String(contentsOfFile: Q + "/f.txt", encoding: .utf8)) == "new\n", "replace: the old target is overwritten")
        check(fm.fileExists(atPath: Q + "/deep/er/d/inner.txt"), "place makes missing parent folders")
        let u = FileOps.undo()
        check(u != nil, "one undo step")
        check((try? String(contentsOfFile: Q + "/f.txt", encoding: .utf8)) == "old\n", "undo puts the replaced file back")
        check(!fm.fileExists(atPath: Q + "/deep"), "undo removes the folders it made")
        check(!FileOps.canUndo, "the whole batch was ONE undo record")

        out = FileOps.place([(P + "/f.txt", Q + "/f.txt")], move: false, clash: .keepBoth)
        check(fm.fileExists(atPath: Q + "/f 2.txt") && (try? String(contentsOfFile: Q + "/f.txt", encoding: .utf8)) == "old\n", "keep both: \" 2\" name")
        _ = FileOps.undo()
        out = FileOps.place([(P + "/f.txt", Q + "/f.txt")], move: false, clash: .skip)
        check(out.changes.isEmpty && !FileOps.canUndo, "skip: nothing done, nothing to undo")

        out = FileOps.place([(P + "/d", Q + "/moved/d")], move: true, clash: .replace)
        check(!fm.fileExists(atPath: P + "/d") && fm.fileExists(atPath: Q + "/moved/d/inner.txt"), "place move")
        _ = FileOps.undo()
        check(fm.fileExists(atPath: P + "/d/inner.txt") && !fm.fileExists(atPath: Q + "/moved"), "undo moves it back and drops the made folder")
        out = FileOps.place([(P + "/d", P + "/d/in")], move: false, clash: .replace)
        check(out.failed != nil, "a folder can't go inside itself")

        let tr = FileOps.trash([Q + "/f.txt"])
        check(tr.failed == nil && !fm.fileExists(atPath: Q + "/f.txt"), "trash")
        _ = FileOps.undo()
        check(fm.fileExists(atPath: Q + "/f.txt"), "undo trash")

        syncAndLinks(tmp)
        print("Folder compare: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
