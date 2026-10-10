// sources: FileOps.swift
import Foundation

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_file_ops.swift:\(line))")
    }
}

let fm = FileManager.default
let root = NSTemporaryDirectory() + "fileops-test-\(getpid())"
var trashedByTests: [String] = []

func exists(_ p: String) -> Bool { fm.fileExists(atPath: root + "/" + p) }
func write(_ p: String, _ text: String = "x") {
    try? fm.createDirectory(atPath: ((root + "/" + p) as NSString).deletingLastPathComponent,
                            withIntermediateDirectories: true)
    try? text.write(toFile: root + "/" + p, atomically: true, encoding: .utf8)
}
func read(_ p: String) -> String? { try? String(contentsOfFile: root + "/" + p, encoding: .utf8) }

func testNames() {
    write("n/a.txt"); write("n/a 2.txt"); write("n/noext"); write("n/.env")
    let dir = URL(fileURLWithPath: root + "/n")
    check(FileOps.freeURL("b.txt", in: dir).lastPathComponent == "b.txt", "a free name is kept")
    check(FileOps.freeURL("a.txt", in: dir).lastPathComponent == "a 3.txt", "a clash counts up past taken names")
    check(FileOps.freeURL("noext", in: dir).lastPathComponent == "noext 2", "no extension")
    check(FileOps.copyURL(of: dir.appendingPathComponent("a.txt")).lastPathComponent == "a copy.txt", "duplicate name")
    write("n/a copy.txt")
    check(FileOps.copyURL(of: dir.appendingPathComponent("a.txt")).lastPathComponent == "a copy 2.txt", "second duplicate")
    check(FileOps.copyURL(of: dir.appendingPathComponent("noext")).lastPathComponent == "noext copy", "duplicate, no extension")
    check(FileOps.copyURL(of: dir.appendingPathComponent(".env")).lastPathComponent == ".env copy", "duplicate of a dotfile")
}

func testCopyAndUndo() {
    FileOps.forgetUndo()
    write("c/src/one.txt", "1"); write("c/src/two.txt", "2"); write("c/dst/one.txt", "old")
    let urls = ["one.txt", "two.txt"].map { URL(fileURLWithPath: root + "/c/src/" + $0) }
    let out = FileOps.transfer(urls, into: root + "/c/dst", move: false)
    check(out.failed == nil && out.changes.count == 2, "both copied")
    check(out.changes.allSatisfy { $0.from == nil }, "a copy reports no origin")
    check(read("c/dst/one.txt") == "old" && read("c/dst/one 2.txt") == "1", "a clash keeps both")
    check(exists("c/src/one.txt") && exists("c/dst/two.txt"), "sources stay")
    let u = FileOps.undo()
    trashedByTests += u?.outcome.paths ?? []
    check(u != nil && !exists("c/dst/one 2.txt") && !exists("c/dst/two.txt"), "undo removes the copies")
    check(read("c/dst/one.txt") == "old", "undo leaves what was already there")
    check(FileOps.undo() == nil, "nothing left to undo")
}

func testMoveAndUndo() {
    FileOps.forgetUndo()
    write("m/src/f.txt", "f"); write("m/src/dir/in.txt", "in")
    try? fm.createDirectory(atPath: root + "/m/dst", withIntermediateDirectories: true)
    let urls = ["f.txt", "dir"].map { URL(fileURLWithPath: root + "/m/src/" + $0) }
    let out = FileOps.transfer(urls, into: root + "/m/dst", move: true)
    check(out.changes.count == 2 && out.changes[0].from == root + "/m/src/f.txt", "a move reports where it was")
    check(!exists("m/src/f.txt") && exists("m/dst/f.txt") && exists("m/dst/dir/in.txt"), "moved, folder and all")
    let same = FileOps.transfer([URL(fileURLWithPath: root + "/m/dst/f.txt")], into: root + "/m/dst", move: true)
    check(same.changes.isEmpty && same.failed == nil, "moving to where it is does nothing")
    let inside = FileOps.transfer([URL(fileURLWithPath: root + "/m/dst/dir")], into: root + "/m/dst/dir", move: true)
    check(inside.changes.isEmpty && inside.failed != nil, "a folder can't move into itself")
    let u = FileOps.undo()
    check(u?.outcome.changes.count == 2 && exists("m/src/f.txt") && exists("m/src/dir/in.txt"), "undo moves back")
    check(!exists("m/dst/f.txt"), "…and out of the destination")
}

func testUndoBlocked() {
    FileOps.forgetUndo()
    write("b/src/f.txt", "new")
    try? fm.createDirectory(atPath: root + "/b/dst", withIntermediateDirectories: true)
    _ = FileOps.transfer([URL(fileURLWithPath: root + "/b/src/f.txt")], into: root + "/b/dst", move: true)
    write("b/src/f.txt", "squatter")
    let u = FileOps.undo()
    check(u?.outcome.changes.isEmpty == true && u?.outcome.failed != nil, "undo never overwrites a file")
    check(read("b/src/f.txt") == "squatter" && read("b/dst/f.txt") == "new", "both files untouched")
}

func testTrashAndUndo() {
    FileOps.forgetUndo()
    write("t/gone.txt", "g"); write("t/dir/in.txt", "in"); write("t/stay.txt")
    let out = FileOps.trash([root + "/t/gone.txt", root + "/t/dir", root + "/t/missing.txt"])
    check(out.changes.count == 2 && out.failed != nil, "two trashed, the missing one reported")
    check(!exists("t/gone.txt") && !exists("t/dir") && exists("t/stay.txt"), "gone from the folder")
    check(out.changes.allSatisfy { fm.fileExists(atPath: $0.to) && $0.to != $0.from }, "…and in the Trash")
    let u = FileOps.undo()
    check(u?.what == "trash of 2 items", "undo names what it takes back")
    check(read("t/gone.txt") == "g" && read("t/dir/in.txt") == "in", "undo puts them back")
    check(out.changes.allSatisfy { !fm.fileExists(atPath: $0.to) }, "…out of the Trash")
}

func testCreateDuplicateRename() {
    FileOps.forgetUndo()
    try? fm.createDirectory(atPath: root + "/n2", withIntermediateDirectories: true)
    let a = FileOps.create("untitled folder", in: root + "/n2", folder: true)
    let b = FileOps.create("untitled folder", in: root + "/n2", folder: true)
    var isDir: ObjCBool = false
    check(fm.fileExists(atPath: a.paths.first ?? "", isDirectory: &isDir) && isDir.boolValue, "new folder")
    check(b.paths.first == root + "/n2/untitled folder 2", "a second one gets a free name")
    let f = FileOps.create("untitled.txt", in: root + "/n2", folder: false)
    check(read("n2/untitled.txt") == "" && f.failed == nil, "new empty file")
    let none = FileOps.create("x", in: root + "/no-such-dir", folder: true)
    check(none.changes.isEmpty && none.failed != nil, "a missing folder is an error, not a crash")

    write("n2/doc.md", "d")
    let d = FileOps.duplicate([root + "/n2/doc.md"])
    check(read("n2/doc copy.md") == "d" && d.changes.first?.from == nil, "duplicate")

    try? fm.moveItem(atPath: root + "/n2/doc.md", toPath: root + "/n2/Doc.md")
    FileOps.recordRename(from: root + "/n2/doc.md", to: root + "/n2/Doc.md")
    let u = FileOps.undo()
    let names = (try? fm.contentsOfDirectory(atPath: root + "/n2")) ?? []
    check(u?.what == "rename of doc.md" && names.contains("doc.md") && !names.contains("Doc.md"),
          "a case-only rename undoes")
    for _ in 0..<4 { trashedByTests += FileOps.undo()?.outcome.paths ?? [] }
    check(!exists("n2/doc copy.md") && !exists("n2/untitled folder") && !exists("n2/untitled.txt"),
          "undo walks back through every op")
    check(!FileOps.canUndo, "the stack is empty")
}

@main
struct FileOpsTests {
    static func main() {
        try? fm.createDirectory(atPath: root, withIntermediateDirectories: true)
        testNames()
        testCopyAndUndo()
        testMoveAndUndo()
        testUndoBlocked()
        testTrashAndUndo()
        testCreateDuplicateRename()
        for p in trashedByTests { try? fm.removeItem(atPath: p) }
        try? fm.removeItem(atPath: root)
        print("\n\(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
