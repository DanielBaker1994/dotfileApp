// sources: RecentFiles.swift ProcessRun.swift
// The file browser's "Recent" list (RecentFiles.swift) must keep a file
// through whatever happens to it: renamed, moved, its folder renamed —
// by this app (own writes: the FSEvents stream ignores them) or by anything
// else (Finder, mv, an editor's atomic save).
// Usage: bin/run-tests.sh recent
//   part 1: synthetic file events → RecentFiles.handle (no stream)
//   part 2: a live stream on a temp "home": real `mv` / `touch` from other
//           processes, and this process's own renames via ownChange

import Foundation
import CoreServices

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_recent_files.swift:\(line))")
    }
}

let fm = FileManager.default
let created = FSEventStreamEventFlags(kFSEventStreamEventFlagItemCreated | kFSEventStreamEventFlagItemIsFile)
let renamed = FSEventStreamEventFlags(kFSEventStreamEventFlagItemRenamed | kFSEventStreamEventFlagItemIsFile)
let renamedDir = FSEventStreamEventFlags(kFSEventStreamEventFlagItemRenamed | kFSEventStreamEventFlagItemIsDir)
let removedDir = FSEventStreamEventFlags(kFSEventStreamEventFlagItemRemoved | kFSEventStreamEventFlagItemIsDir)

// FSEvents reports real paths (/private/var/…, which NSString's
// resolvingSymlinksInPath strips)
let tmpRoot: String = {
    guard let c = realpath(NSTemporaryDirectory(), nil) else { return NSTemporaryDirectory() }
    defer { free(c) }
    return String(cString: c)
}()

// a fresh "home"
func makeHome(_ tag: String) -> String {
    let base = tmpRoot
    let dir = base + "/recent-test-\(tag)-\(getpid())"
    try? fm.removeItem(atPath: dir)
    try! fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
    return dir
}

func write(_ p: String, _ text: String = "x") {
    try? fm.createDirectory(atPath: (p as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
    try! text.write(toFile: p, atomically: false, encoding: .utf8)
}

func mv(_ a: String, _ b: String) { try! fm.moveItem(atPath: a, toPath: b) }

// another process does it (the stream only ignores THIS process)
func sh(_ exe: String, _ args: String...) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: exe)
    p.arguments = args
    try! p.run()
    p.waitUntilExit()
}

func paths(_ r: RecentFiles) -> [String] { r.entries().map { $0.path } }

@discardableResult
func waitFor(_ seconds: Double = 8, _ cond: () -> Bool) -> Bool {
    let end = Date().addingTimeInterval(seconds)
    while Date() < end {
        if cond() { return true }
        Thread.sleep(forTimeInterval: 0.1)
    }
    return cond()
}

// MARK: - part 1: synthetic events

func newUnit(_ tag: String) -> (RecentFiles, String) {
    let home = makeHome(tag)
    let r = RecentFiles(home: home, store: home + "/store/recent.json")
    r.configure(enabled: false, days: 7, limit: 200, excludes: [], everywhere: false)
    return (r, home)
}

func testCreated() {
    print("A new file is listed:")
    let (r, home) = newUnit("created")
    write(home + "/a.txt")
    r.handle([home + "/a.txt"], [created], [1])
    check(paths(r) == [home + "/a.txt"], "created file is listed")
    write(home + "/.hidden")
    write(home + "/x.crdownload")
    r.handle([home + "/.hidden", home + "/x.crdownload"], [created, created], [2, 3])
    check(paths(r) == [home + "/a.txt"], "hidden files / partial downloads are not")
}

func testRenameFile() {
    print("Renamed file (by another app):")
    let (r, home) = newUnit("rename")
    let a = home + "/a.txt", b = home + "/b.txt"
    write(a)
    r.handle([a], [created], [1])
    mv(a, b)
    r.handle([a, b], [renamed, renamed], [1, 1])
    check(paths(r) == [b], "listed under its new name only")

    // the two halves in separate callbacks
    let c = home + "/c.txt"
    mv(b, c)
    r.handle([b], [renamed], [1])
    check(paths(r) == [], "old name leaves at once")
    r.handle([c], [renamed], [1])
    check(paths(r) == [c], "new name arrives with the second callback")

    // no inode known: still "old gone, new appeared"
    let d = home + "/d.txt"
    mv(c, d)
    r.handle([c, d], [renamed, renamed])
    check(paths(r) == [d], "works without inodes")
}

func testCaseOnlyRename() {
    print("Case-only rename:")
    let (r, home) = newUnit("case")
    let a = home + "/note.txt", b = home + "/Note.txt"
    write(a)
    r.handle([a], [created], [1])
    guard fm.fileExists(atPath: home + "/NOTE.TXT") else {
        print("  (case-sensitive volume: skipped)")
        return
    }
    mv(a, b)
    r.handle([a, b], [renamed, renamed], [1, 1])
    check(paths(r) == [b], "listed once, under the new spelling — got \(paths(r).map { ($0 as NSString).lastPathComponent })")
}

func testRenameFolder() {
    print("Renamed folder keeps the files listed inside it:")
    let (r, home) = newUnit("folder")
    let d = home + "/proj", e = home + "/project"
    write(d + "/one.txt")
    write(d + "/sub/two.txt")
    r.handle([d + "/one.txt", d + "/sub/two.txt"], [created, created], [1, 2])
    // a folder whose name only STARTS the same must not be touched
    write(home + "/proj2/three.txt")
    r.handle([home + "/proj2/three.txt"], [created], [3])
    mv(d, e)
    r.handle([d, e], [renamedDir, renamedDir], [9, 9])
    let got = Set(paths(r))
    check(got.contains(e + "/one.txt"), "file follows the folder")
    check(got.contains(e + "/sub/two.txt"), "nested file follows the folder")
    check(got.contains(home + "/proj2/three.txt"), "sibling with the same prefix is untouched")
    check(!got.contains { $0.hasPrefix(d + "/") }, "nothing left under the old name")
    check(got.contains(e), "the renamed folder itself is listed")
}

func testRenameOutOfScope() {
    print("Renamed out of scope / unrelated renames:")
    let (r, home) = newUnit("scope")
    let d = home + "/keep"
    write(d + "/one.txt")
    r.handle([d + "/one.txt"], [created], [1])
    // into a hidden folder: only the old half is ever seen
    mv(d, home + "/.trash")
    r.handle([d, home + "/.trash"], [renamedDir, renamedDir], [9, 9])
    check(paths(r) == [], "hidden → gone from the list")
    // a different folder renamed next must not inherit those files
    write(home + "/other/x.txt")
    mv(home + "/other", home + "/other2")
    r.handle([home + "/other", home + "/other2"], [renamedDir, renamedDir], [7, 7])
    check(paths(r) == [home + "/other2"], "unrelated rename inherits nothing — got \(paths(r))")
}

func testRemovedFolder() {
    print("Deleted folder:")
    let (r, home) = newUnit("removed")
    let d = home + "/gone"
    write(d + "/one.txt")
    write(home + "/stay.txt")
    r.handle([d + "/one.txt", home + "/stay.txt"], [created, created], [1, 2])
    try! fm.removeItem(atPath: d)
    r.handle([d], [removedDir], [9])
    check(paths(r) == [home + "/stay.txt"], "its files leave the list, others stay")
}

func testAtomicSave() {
    print("Editor saves (temp file renamed over the original):")
    let (r, home) = newUnit("atomic")
    let a = home + "/doc.md", t = home + "/doc.md.sb-1234"
    write(a)
    r.handle([a], [created], [1])
    write(t, "new")
    r.handle([t], [created], [2])
    _ = try! fm.replaceItemAt(URL(fileURLWithPath: a), withItemAt: URL(fileURLWithPath: t))
    r.handle([t, a], [renamed, renamed], [2, 2])
    check(paths(r) == [a], "the document stays, the temp file never lingers — got \(paths(r))")

    // vim-style: original → backup~, new file written under the old name
    mv(a, a + "~")
    write(a, "newer")
    r.handle([a, a + "~"], [renamed | created, renamed], [3, 1])
    check(paths(r) == [a], "backup-then-rewrite keeps the document")
}

// MARK: - part 2: live stream

func testLive() {
    let home = makeHome("live")
    let store = home + "/store/recent.json"
    let r = RecentFiles(home: home, store: store)
    r.configure(enabled: true, days: 7, limit: 200, excludes: [], everywhere: false)
    func mine() -> [String] { paths(r).filter { $0.hasPrefix(home + "/") } }
    func name(_ p: [String]) -> [String] { p.map { String($0.dropFirst(home.count + 1)) } }
    Thread.sleep(forTimeInterval: 1.0)      // stream up, seed done

    print("Live: a file made by another process:")
    sh("/usr/bin/touch", home + "/a.txt")
    guard waitFor(8, { mine() == [home + "/a.txt"] }) else {
        check(false, "file events arrive — got \(name(mine())) (no FSEvents here? live tests skipped)")
        return
    }
    check(true, "is listed")

    print("Live: renamed by another process (mv):")
    sh("/bin/mv", home + "/a.txt", home + "/b.txt")
    check(waitFor { mine() == [home + "/b.txt"] }, "listed under the new name — got \(name(mine()))")

    print("Live: its folder renamed by another process:")
    write(home + "/dir/keep.txt")           // our own write: invisible to the stream…
    r.ownChange(from: nil, to: home + "/dir/keep.txt")   // …so it's reported
    check(waitFor { mine().contains(home + "/dir/keep.txt") }, "own new file is listed")
    sh("/bin/mv", home + "/dir", home + "/dir2")
    check(waitFor { mine().contains(home + "/dir2/keep.txt") },
          "file follows the renamed folder — got \(name(mine()))")
    check(!mine().contains(home + "/dir/keep.txt"), "old path is gone")

    print("Live: renamed by THIS app (the reported bug):")
    let before = mine()
    let at = before.firstIndex(of: home + "/b.txt")
    mv(home + "/b.txt", home + "/c.txt")
    r.ownChange(from: home + "/b.txt", to: home + "/c.txt")
    // no waiting: the browser reloads its list on the very next line
    check(mine().contains(home + "/c.txt"), "new name is there at once — got \(name(mine()))")
    check(!mine().contains(home + "/b.txt"), "old name is gone at once")
    check(mine().firstIndex(of: home + "/c.txt") == at, "the row keeps its place in the list")
    Thread.sleep(forTimeInterval: 1.5)
    check(mine().contains(home + "/c.txt") && mine().count == before.count,
          "and stays once the store caught up — got \(name(mine()))")
    check(mine().firstIndex(of: home + "/c.txt") == at, "still in its place")

    print("Live: folder renamed by this app:")
    mv(home + "/dir2", home + "/dir3")
    r.ownChange(from: home + "/dir2", to: home + "/dir3")
    check(mine().contains(home + "/dir3/keep.txt"), "file inside follows at once — got \(name(mine()))")
    Thread.sleep(forTimeInterval: 0.5)
    check(mine().contains(home + "/dir3/keep.txt") && !mine().contains { $0.hasPrefix(home + "/dir2") },
          "and stays")

    print("Live: moved / copied by this app (drag & drop):")
    write(home + "/unlisted.txt")           // never reported: not in the list
    check(!mine().contains(home + "/unlisted.txt"), "own writes are not seen by the stream")
    mv(home + "/unlisted.txt", home + "/dir3/moved.txt")
    r.ownChange(from: home + "/unlisted.txt", to: home + "/dir3/moved.txt")
    check(waitFor { mine().first == home + "/dir3/moved.txt" }, "a moved file shows up, newest first")
    try! fm.copyItem(atPath: home + "/c.txt", toPath: home + "/c 2.txt")
    r.ownChange(from: nil, to: home + "/c 2.txt")
    check(waitFor { mine().first == home + "/c 2.txt" }, "a copy shows up, newest first")
    check(mine().contains(home + "/c.txt"), "the original stays")

    print("Live: renamed to a hidden name by this app:")
    mv(home + "/c 2.txt", home + "/.c2")
    r.ownChange(from: home + "/c 2.txt", to: home + "/.c2")
    check(waitFor { !mine().contains { $0.hasSuffix("c2") || $0.hasSuffix("c 2.txt") } }, "leaves the list")

    print("Live: recent.json (survives a relaunch):")
    let saved = waitFor(6) {
        guard let d = try? Data(contentsOf: URL(fileURLWithPath: store)),
              let arr = (try? JSONSerialization.jsonObject(with: d)) as? [[String: Any]] else { return false }
        let ps = arr.compactMap { $0["path"] as? String }
        return ps.contains(home + "/c.txt") && ps.contains(home + "/dir3/keep.txt")
            && !ps.contains(home + "/b.txt") && !ps.contains { $0.hasPrefix(home + "/dir2/") }
    }
    check(saved, "holds the new names, not the old ones")
    r.configure(enabled: false, days: 7, limit: 200, excludes: [], everywhere: false)
    let again = RecentFiles(home: home, store: store)
    again.configure(enabled: true, days: 7, limit: 200, excludes: [], everywhere: false)
    check(waitFor { paths(again).contains(home + "/c.txt") && paths(again).contains(home + "/dir3/keep.txt") },
          "a new instance lists the renamed files")
    again.configure(enabled: false, days: 7, limit: 200, excludes: [], everywhere: false)
}

@main
struct RecentFilesTests {
    static func main() {
        testCreated()
        testRenameFile()
        testCaseOnlyRename()
        testRenameFolder()
        testRenameOutOfScope()
        testRemovedFolder()
        testAtomicSave()
        testLive()
        let base = tmpRoot
        for n in (try? fm.contentsOfDirectory(atPath: base)) ?? []
        where n.hasPrefix("recent-test-") && n.hasSuffix("-\(getpid())") {
            try? fm.removeItem(atPath: base + "/" + n)
        }
        print("\n\(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
