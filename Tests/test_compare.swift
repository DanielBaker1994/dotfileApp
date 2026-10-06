// sources: CompareText.swift
// The Compare view's text engine (CompareText.swift): byte-exact round
// trips, importance, rows + sections, copy across + undo, the windowed
// re-diff, and hunk parity with `git diff --no-index --histogram` on a
// corpus of real file pairs (KR4) + timings (KR2, engine part).
// Usage: bin/run-tests.sh compare

import Foundation

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_compare.swift:\(line))")
    }
}

let fm = FileManager.default
let root = (CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : nil)
    ?? (URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path)
let tmp: String = {
    let t = NSTemporaryDirectory() + "compare-test-\(getpid())"
    try? FileManager.default.createDirectory(atPath: t, withIntermediateDirectories: true)
    return t
}()

func ms(_ t0: UInt64) -> Double { Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000 }

// MARK: round trips

func testRoundTrips() {
    func bytes(_ s: String) -> Data { Data(s.utf8) }
    var cases: [(String, Data, TextEncodingKind)] = [
        ("utf8 lf", bytes("a\nb\nc\n"), .utf8),
        ("utf8 no final newline", bytes("a\nb\nc"), .utf8),
        ("crlf", bytes("a\r\nb\r\n"), .utf8),
        ("cr", bytes("a\rb\r"), .utf8),
        ("mixed endings", bytes("a\r\nb\nc\rd"), .utf8),
        ("blank lines", bytes("\n\n\n"), .utf8),
        ("empty", Data(), .utf8),
        ("unicode", bytes("héllo wörld\n日本語\n😀 emoji\r\n"), .utf8),
        ("nfd", bytes("e\u{301}\n"), .utf8),
        ("utf8 bom", Data([0xEF, 0xBB, 0xBF]) + bytes("x = 1\r\ny = 2\r\n"), .utf8BOM),
        ("latin1", Data([0x63, 0x61, 0x66, 0xE9, 0x0A, 0xFF, 0x0A]), .latin1),
    ]
    let u16 = "line one\r\nline two\nsmile 😀\n"
    cases.append(("utf16 le", Data([0xFF, 0xFE]) + u16.data(using: .utf16LittleEndian)!, .utf16LE))
    cases.append(("utf16 be", Data([0xFE, 0xFF]) + u16.data(using: .utf16BigEndian)!, .utf16BE))
    for (name, data, enc) in cases {
        guard let side = TextSide.decode(data) else { check(false, "\(name): decoded as binary"); continue }
        check(side.encoding == enc, "\(name): encoding \(side.encoding)")
        check(side.encoded() == data, "\(name): decode → encode is byte-exact")
        // an edit taken back saves the same bytes again
        var t = TextCompare(left: side, right: side)
        if !side.lines.isEmpty {
            t.replace(.left, 0..<1, with: ["changed", "two"])
            t.undo()
            check(t.left.encoded() == data, "\(name): edit + undo is byte-exact")
        }
        // saved to disk and read back
        let p = tmp + "/rt-\(name.replacingOccurrences(of: " ", with: "-"))"
        try? side.encoded()?.write(to: URL(fileURLWithPath: p))
        check((try? Data(contentsOf: URL(fileURLWithPath: p))) == data, "\(name): save round trip")
    }
    check(TextSide.isBinary(Data([0x50, 0x4B, 0x03, 0x04, 0x00, 0x00])), "NUL bytes = binary")
    check(!TextSide.isBinary(Data([0xFF, 0xFE, 0x41, 0x00])), "UTF-16 with a BOM is text")
    // a line keeps its file's endings; text appended after a last line
    // without newline gives it one, the new last line keeps "no newline"
    var s = TextSide.decode(bytes("a\r\nb"))!
    _ = s.replace(2..<2, with: ["c"])
    check(s.encoded() == bytes("a\r\nb\r\nc"), "append after no-newline last line: \(s.text.debugDescription)")
    var s2 = TextSide.decode(bytes("a\r\nb\r\n"))!
    _ = s2.replace(1..<2, with: ["x", "y"])
    check(s2.encoded() == bytes("a\r\nx\r\ny\r\n"), "new lines take the side's CRLF")
    // Latin-1 can't hold every character: nil = save as UTF-8
    var l1 = TextSide.decode(Data([0x61, 0xE9, 0x0A]))!
    _ = l1.replace(0..<1, with: ["日本"])
    check(l1.encoded() == nil, "Latin-1 refuses characters it can't hold")
}

// MARK: importance + rows

func side(_ s: String) -> TextSide { TextSide(text: s) }

func testImportance() {
    let imp = Importance()
    check(imp.normalized("  foo bar  ") == "foo bar", "leading + trailing ws dropped by default")
    var e = imp
    e.embeddedWS = true
    check(e.normalized("  a  b\tc ") == "abc", "embedded ws dropped")
    e.leadingWS = false
    check(e.normalized("  a  b ") == "  ab", "leading kept when it counts")
    var c = Importance.exact
    c.ignoreCase = true
    check(c.normalized("FooBAR") == "foobar", "case")
    check(Importance().key("x", .crlf) == Importance().key("x", .lf), "line endings unimportant by default")
    check(Importance.exact.key("x", .crlf) != Importance.exact.key("x", .lf), "exact: line endings count")

    // whitespace-only difference = unimportant (blue), not a red section
    var t = TextCompare(left: side("a\n  b\nc\n"), right: side("a\nb\nc\n"))
    check(t.rows.count == 3 && t.rows[1].kind == .changed && !t.rows[1].important, "indent-only change is unimportant")
    check(t.sections.count == 1 && !t.sections[0].important, "one unimportant section")
    t.setIgnoreUnimportant(true)
    check(t.sections.isEmpty, "Ignore Unimportant: no sections")

    // blank lines
    var bl = Importance()
    bl.blankLines = true
    let tb = TextCompare(left: side("a\n\nb\n"), right: side("a\nb\n"), importance: bl)
    check(tb.sections.count == 1 && !tb.sections[0].important, "inserted blank line is unimportant with blank-lines on")
    let tb2 = TextCompare(left: side("a\n\nb\n"), right: side("a\nb\n"))
    check(tb2.sections.count == 1 && tb2.sections[0].important, "…and important by default")

    // CRLF vs LF: same text, but the files are not "identical"
    let crlf = TextCompare(left: side("a\r\nb\r\n"), right: side("a\nb\n"))
    check(crlf.identicalText, "CRLF vs LF reads as the same text by default")
    check(crlf.left.encoded() != crlf.right.encoded(), "…while the bytes differ (status says so)")
    let crlfExact = TextCompare(left: side("a\r\nb\r\n"), right: side("a\nb\n"), importance: .exact)
    check(!crlfExact.identicalText, "exact: CRLF vs LF differ")
    // NFC vs NFD: never "identical"
    let nf = TextCompare(left: side("caf\u{E9}\n"), right: side("cafe\u{301}\n"), importance: .exact)
    check(!nf.identicalText, "NFC vs NFD bytes differ → not identical")
}

func testRows() {
    let t = TextCompare(left: side("one\ntwo\nthree\nfour\n"), right: side("one\nTWO\nthree\nfour\nfive\n"))
    check(t.rows.map(\.kind) == [.same, .changed, .same, .same, .rightOnly], "rows: \(t.rows.map(\.kind))")
    check(t.sections.count == 2, "two sections")
    check(t.nextSection(after: 0) == 0 && t.nextSection(after: 1) == 1 && t.prevSection(before: 4) == 0, "next / prev")
    // fillers line up a deleted block
    let d = TextCompare(left: side("a\nb\nc\nd\n"), right: side("a\nd\n"))
    check(d.rows.map(\.kind) == [.same, .leftOnly, .leftOnly, .same], "deleted lines get fillers")
    // similar lines pair up across a count mismatch
    let p = TextCompare(left: side("x\nport = 8080\ny\n"),
                        right: side("x\nnew line here\nport = 9090\ny\n"))
    let ports = p.rows.first { $0.l == 1 }
    check(ports?.r == 2 && ports?.kind == .changed, "port lines paired: \(p.rows)")
    // context filter
    var big = "", big2 = ""
    for i in 0..<40 { big += "line \(i)\n"; big2 += (i == 20 ? "LINE 20" : "line \(i)") + "\n" }
    let c = TextCompare(left: side(big), right: side(big2))
    check(c.visibleRows(.context, context: 3) == Array(17...23), "context 3 around one change")
    check(c.visibleRows(.diffs, context: 3) == [20], "diffs only")
    check(c.visibleRows(.same, context: 3)?.count == 39, "same only")
    // char marks
    let m = CharDiff.marks("port = 8080", "port = 9090", Importance())
    check(m.left == [CharDiff.Mark(range: NSRange(location: 7, length: 4), important: true)], "char mark left \(m.left)")
    check(m.right.first?.range == NSRange(location: 7, length: 4), "char mark right")
    let ws = CharDiff.marks("a  b", "a b", Importance())
    check(ws.left.allSatisfy { !$0.important }, "spacing marks are unimportant")
    // the AI view's word diff (moved here): unchanged behavior
    let ops = CharDiff.diff("their going home", "They're gone home")
    check(CharDiff.changes(ops) == 1, "AI word diff: one replacement")
    // binary
    check(BinaryCompare.firstDifference(Data([1, 2, 3]), Data([1, 2, 3])) == nil, "binary identical")
    check(BinaryCompare.firstDifference(Data(repeating: 7, count: 100) + Data([1]), Data(repeating: 7, count: 100) + Data([2])) == 100,
          "binary first difference")
    check(BinaryCompare.firstDifference(Data([1, 2]), Data([1, 2, 3])) == 2, "binary length difference")
}

// rows must spell out both files in order, and same rows hold equal text
func consistent(_ t: TextCompare) -> Bool {
    var l = 0, r = 0
    for row in t.rows {
        if row.l >= 0 { if Int(row.l) != l { return false }; l += 1 }
        if row.r >= 0 { if Int(row.r) != r { return false }; r += 1 }
        if row.kind == .same, row.l < 0 || row.r < 0 || t.left.lines[Int(row.l)] != t.right.lines[Int(row.r)] { return false }
        if row.kind == .leftOnly && (row.l < 0 || row.r >= 0) { return false }
        if row.kind == .rightOnly && (row.r < 0 || row.l >= 0) { return false }
    }
    return l == t.left.lines.count && r == t.right.lines.count
}

// MARK: copy across + undo + windowed re-diff

func testEdits() {
    let L = "a\nb\nc\nd\ne\nf\n", R = "a\nB\nc\nd\nx\ny\nf\n"
    var t = TextCompare(left: side(L), right: side(R))
    check(t.sections.count == 2, "two sections before copying")
    let before = t.right.encoded()
    t.copySection(0, from: .left)
    check(t.right.lines == ["a", "b", "c", "d", "x", "y", "f"], "copy section 0 to the right: \(t.right.lines)")
    check(t.sections.count == 1, "one section left")
    t.copySection(0, from: .right)
    check(t.left.lines == ["a", "b", "c", "d", "x", "y", "f"], "copy section to the left: \(t.left.lines)")
    check(t.sections.isEmpty && t.identicalText, "identical after both copies")
    t.undo()
    check(t.left.lines == ["a", "b", "c", "d", "e", "f"], "undo the left copy")
    t.undo()
    check(t.right.encoded() == before, "undo the right copy: byte-exact")
    t.redo()
    check(t.right.lines[1] == "b", "redo")
    check(consistent(t), "rows consistent after undo / redo")
    // undo of a side's own newest edit while the other side has newer ones
    var u = TextCompare(left: side("1\n2\n3\n"), right: side("1\n2\n3\n"))
    u.replace(.left, 1..<2, with: ["two"])
    u.replace(.right, 0..<1, with: ["uno", "one"])
    u.undo(.left)
    check(u.left.lines == ["1", "2", "3"] && u.right.lines == ["uno", "one", "2", "3"], "per-side undo")
    // copy one row (a line) across, a filler row = delete
    var c = TextCompare(left: side("a\nb\nc\n"), right: side("a\nc\n"))
    let fillerRow = c.rows.firstIndex { $0.kind == .leftOnly }!
    c.copyRows(fillerRow..<(fillerRow + 1), from: .right)
    check(c.left.lines == ["a", "c"], "copying a filler deletes the line")

    // fuzz: random edits, the windowed re-diff keeps rows consistent and
    // (mostly) equal to a full diff
    var rng = SplitMix(seed: 42)
    var base: [String] = []
    for i in 0..<400 { base.append(i % 7 == 0 ? "" : "    let v\(i % 50) = compute(\(i))") }
    var f = TextCompare(left: TextSide(text: base.joined(separator: "\n") + "\n"),
                        right: TextSide(text: base.joined(separator: "\n") + "\n"))
    var sameAsFull = 0, edits = 0
    for _ in 0..<150 {
        let s: CompareSide = rng.next() % 2 == 0 ? .left : .right
        let n = f.side(s).lines.count
        let a = Int(rng.next() % UInt64(max(1, n)))
        let len = Int(rng.next() % 4)
        let r = a..<min(n, a + len)
        var new: [String] = []
        for _ in 0..<Int(rng.next() % 4) { new.append("edit \(rng.next() % 1000)") }
        f.replace(s, r, with: new)
        edits += 1
        if !consistent(f) { check(false, "fuzz: rows inconsistent after edit \(edits)"); break }
        let full = TextCompare(left: f.left, right: f.right)
        if full.rows == f.rows { sameAsFull += 1 }
        if rng.next() % 5 == 0 {
            f.undo()
            if !consistent(f) { check(false, "fuzz: rows inconsistent after undo"); break }
        }
    }
    check(sameAsFull * 100 >= edits * 90, "windowed re-diff = full diff in \(sameAsFull)/\(edits) edits")
    print("  windowed re-diff matched a full diff after \(sameAsFull)/\(edits) random edits")
}

// MARK: phase 3: Align With, trim trailing whitespace, line endings

func testPhase3() {
    // Align With: force left "x" onto right "y" (the diff would leave them apart)
    var a = TextCompare(left: side("a\nx\nb\nc\n"), right: side("a\nb\nc\ny\n"))
    let before = a.rows.count
    a.align(left: 1, right: 3)
    let row = a.rows.firstIndex { $0.l == 1 }!
    check(a.rows[row].r == 3 && a.rows[row].kind == .changed, "align: left 1 sits on right 3")
    check(a.isAnchor(row: row), "align: the row is an anchor")
    check(consistent(a), "align: rows consistent")
    check(a.rows.count >= before, "align: fillers around the anchor")
    // an edit above the anchor moves it; one on it drops it
    a.replace(.left, 0..<0, with: ["new"])
    check(a.anchors.first.map { $0.l == 2 && $0.r == 3 } == true, "align: the anchor follows an edit above it")
    check(consistent(a), "align: rows consistent after an edit")
    a.undo()
    check(a.anchors.first.map { $0.l == 1 } == true, "align: undo moves it back")
    a.replace(.left, 0..<3, lines: ["A", "X", "B"], eols: [.lf, .lf, .lf])
    check(a.anchors.first.map { $0.l == 1 && $0.r == 3 } == true, "align: a line-for-line rewrite keeps the anchor")
    a.replace(.left, 1..<2, with: [])
    check(a.anchors.isEmpty, "align: deleting the anchored line drops it")
    a.clearAlignment()
    check(a.anchors.isEmpty && a.rows == TextCompare(left: a.left, right: a.right).rows, "align: cleared = the plain diff")
    // a crossing anchor is replaced
    var c = TextCompare(left: side("1\n2\n3\n"), right: side("1\n2\n3\n"))
    c.align(left: 0, right: 2)
    c.align(left: 2, right: 0)
    check(c.anchors.count == 1 && c.anchors[0].l == 2, "align: a crossing anchor is dropped")
    check(consistent(c), "align: crossing rows consistent")
    var w = TextCompare(left: side("1\n2\n"), right: side("1\n2\n"))
    w.align(left: 1, right: 0)
    w.swapSides()
    check(w.anchors.first.map { $0.l == 0 && $0.r == 1 } == true, "align: swap sides flips the anchor")

    // trim trailing whitespace: one undo step, byte-exact undo
    let raw = "a  \r\nb\t\r\nc\r\nd "
    var t = TextCompare(left: TextSide.decode(Data(raw.utf8))!, right: side("a\nb\nc\nd\n"))
    let orig = t.left.encoded()
    check(t.trimTrailingWhitespace(.left) == 3, "trim: three lines changed")
    check(t.left.lines == ["a", "b", "c", "d"] && t.left.eols == [.crlf, .crlf, .crlf, .none], "trim keeps the line endings")
    check(t.trimTrailingWhitespace(.left) == 0, "trim twice: nothing")
    t.undo()
    check(t.left.encoded() == orig, "trim undo: byte-exact")
    // line endings
    check(t.convertLineEndings(.left, to: .lf) == 3, "convert: three endings")
    check(t.left.eols == [.lf, .lf, .lf, .none], "convert keeps a last line without newline")
    check(consistent(t), "convert: rows consistent")
    t.undo()
    check(t.left.encoded() == orig, "convert undo: byte-exact")
}

struct SplitMix {
    var state: UInt64
    init(seed: UInt64) { state = seed }
    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
    mutating func below(_ n: Int) -> Int { n <= 0 ? 0 : Int(next() % UInt64(n)) }
}

// MARK: git parity (KR4)

func run(_ argv: [String], cwd: String? = nil) -> (Int32, String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: argv[0])
    p.arguments = Array(argv.dropFirst())
    if let cwd { p.currentDirectoryURL = URL(fileURLWithPath: cwd) }
    let out = Pipe()
    p.standardOutput = out
    p.standardError = FileHandle.nullDevice
    // the user's git config must not change the answer (diff.algorithm…)
    var env = ProcessInfo.processInfo.environment
    env["GIT_CONFIG_GLOBAL"] = "/dev/null"
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    p.environment = env
    do { try p.run() } catch { return (-1, "") }
    let data = out.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(decoding: data, as: UTF8.self))
}

// "@@ -a,b +c,d @@" (-U0) → hunks in 0-based starts
func gitHunks(_ a: String, _ b: String) -> [LineDiff.Hunk]? {
    let (code, out) = run(["/usr/bin/git", "diff", "--no-index", "--histogram", "--indent-heuristic", "-U0",
                           "--no-color", "--no-ext-diff", a, b])
    guard code == 0 || code == 1 else { return nil }
    var hs: [LineDiff.Hunk] = []
    for line in out.split(separator: "\n") where line.hasPrefix("@@ ") {
        let parts = line.split(separator: " ")
        guard parts.count >= 3 else { continue }
        func range(_ s: Substring) -> (Int, Int) {
            let nums = s.dropFirst().split(separator: ",").map { Int($0) ?? 0 }
            return (nums[0], nums.count > 1 ? nums[1] : 1)
        }
        let (a0, n) = range(parts[1]), (b0, m) = range(parts[2])
        hs.append(LineDiff.Hunk(a: n == 0 ? a0 : a0 - 1, n: n, b: m == 0 ? b0 : b0 - 1, m: m))
    }
    return hs
}

func ourHunks(_ a: Data, _ b: Data) -> [LineDiff.Hunk] {
    let l = TextSide.decode(a)!, r = TextSide.decode(b)!
    var ids: [String: Int32] = [:]
    func key(_ t: TextSide) -> [Int32] {
        t.lines.indices.map { i in
            let k = Importance.exact.key(t.lines[i], t.eols[i])
            if let v = ids[k] { return v }
            let v = Int32(ids.count)
            ids[k] = v
            return v
        }
    }
    return LineDiff.hunks(key(l), key(r), textA: l.lines, textB: r.lines)
}

// seeded mutations of a real file: block delete / insert / copy (repeats),
// edits, moves, re-indents, blank lines — what real edits look like
func mutate(_ lines: [String], _ rng: inout SplitMix) -> [String] {
    var out = lines
    for _ in 0..<(1 + rng.below(6)) {
        let n = out.count
        guard n > 2 else { break }
        let at = rng.below(n), len = 1 + rng.below(min(12, n - at))
        switch rng.below(7) {
        case 0: out.removeSubrange(at..<min(n, at + len))
        case 1:
            let src = rng.below(n)
            out.insert(contentsOf: out[src..<min(n, src + len)], at: at)
        case 2:
            for i in at..<min(n, at + len) where rng.below(2) == 0 { out[i] = out[i].replacingOccurrences(of: "e", with: "E") + " // x" }
        case 3:
            let block = Array(out[at..<min(n, at + len)])
            out.removeSubrange(at..<min(n, at + len))
            out.insert(contentsOf: block, at: rng.below(out.count + 1))
        case 4:
            for i in at..<min(n, at + len) { out[i] = "    " + out[i] }
        case 5: out.insert(contentsOf: Array(repeating: "", count: 1 + rng.below(3)), at: at)
        default:
            out.insert(contentsOf: (0..<len).map { "new line \($0) \(rng.below(100))" }, at: at)
        }
    }
    return out
}

func testParity() {
    var pairs: [(String, Data, Data)] = []
    // 1. seeded mutations of this repo's own files
    let exts = ["swift", "py", "sh", "md", "toml", "vim"]
    let top: [String] = ((try? fm.contentsOfDirectory(atPath: root)) ?? []).sorted()
        .filter { exts.contains(($0 as NSString).pathExtension) }.map { root + "/" + $0 }
    let jira: [String] = ((try? fm.contentsOfDirectory(atPath: root + "/jira")) ?? []).sorted()
        .filter { $0.hasSuffix(".py") }.map { root + "/jira/" + $0 }
    let files = top + jira
    var rng = SplitMix(seed: 2026)
    for f in files {
        guard let d = fm.contents(atPath: f), let s = TextSide.decode(d), s.lines.count > 5,
              s.lines.count < 20_000, !s.eols.contains(EOL.cr) else { continue }
        for k in 0..<2 {
            let m = mutate(s.lines, &rng)
            let text = m.joined(separator: "\n") + (s.eols.last == EOL.none ? "" : "\n")
            pairs.append(("\((f as NSString).lastPathComponent) #\(k)", d, Data(text.utf8)))
        }
        if pairs.count >= 80 { break }
    }
    // 2. consecutive versions from this repo's history (read-only git)
    let (_, log) = run(["/usr/bin/git", "log", "--format=%H", "-n", "40", "--", "PopupWindow.swift",
                        "kitchen_sink.swift", "SharedWindow.swift", "Confluence.swift", "AIWindow.swift"], cwd: root)
    let commits = log.split(separator: "\n").map(String.init)
    var history = 0
    outer: for name in ["SharedWindow.swift", "AIWindow.swift", "Confluence.swift", "CardWindow.swift", "PathShelf.swift"] {
        var prev: Data?
        for c in commits.prefix(30) {
            let (code, text) = run(["/usr/bin/git", "show", "\(c):\(name)"], cwd: root)
            guard code == 0 else { continue }
            let d = Data(text.utf8)
            if let p = prev, p != d {
                pairs.append(("\(name)@\(c.prefix(7))", d, p))
                history += 1
                if history >= 30 { break outer }
            }
            prev = d
        }
    }
    var same = 0
    var shown = 0
    var tDiff = 0.0
    var gitTotal = 0
    for (i, (name, a, b)) in pairs.enumerated() {
        let pa = tmp + "/p\(i)-a", pb = tmp + "/p\(i)-b"
        fm.createFile(atPath: pa, contents: a)
        fm.createFile(atPath: pb, contents: b)
        guard let g = gitHunks(pa, pb) else { check(false, "git diff failed for \(name)"); continue }
        gitTotal += g.count
        let t0 = DispatchTime.now().uptimeNanoseconds
        let o = ourHunks(a, b)
        tDiff += ms(t0)
        if g == o { same += 1 } else if shown < 5 {
            shown += 1
            print("  parity miss \(name): git \(g.count) hunks, ours \(o.count)"
                  + (g.count == o.count ? "" : "") + " first diff: git \(g.first { !o.contains($0) }.map { "\($0)" } ?? "-") ours \(o.first { !g.contains($0) }.map { "\($0)" } ?? "-")")
        }
        // never "identical" for files that differ
        if a != b {
            let t = TextCompare(left: TextSide.decode(a)!, right: TextSide.decode(b)!, importance: .exact)
            check(!t.identicalText, "\(name): differing files never read as identical")
        }
    }
    let pct = pairs.isEmpty ? 0 : Double(same) * 100 / Double(pairs.count)
    print(String(format: "  KR4 parity with git --histogram: %d/%d pairs (%.1f %%, %d git hunks), %d from history; engine %.0f ms total",
                 same, pairs.count, pct, gitTotal, history, tDiff))
    check(gitTotal > pairs.count, "the corpus has real differences (\(gitTotal) hunks)")
    check(pairs.count >= 50, "corpus has ≥ 50 pairs (\(pairs.count))")
    check(pct >= 95, "≥ 95 % hunk parity with git (\(pct))")
}

// MARK: timings (KR2, the engine's share)

func synthetic(_ n: Int, seed: UInt64, change: Int) -> (String, String) {
    var rng = SplitMix(seed: seed)
    var a: [String] = [], b: [String] = []
    a.reserveCapacity(n)
    for i in 0..<n {
        let indent = String(repeating: " ", count: 4 * rng.below(4))
        let line = rng.below(9) == 0 ? "" : "\(indent)let value\(rng.below(500)) = item[\(i % 997)] + \(rng.below(100))"
        a.append(line)
        switch rng.below(change) {
        case 0: b.append(line + " // changed")
        case 1: break
        case 2: b.append(line); b.append("\(indent)inserted(\(i))")
        default: b.append(line)
        }
    }
    return (a.joined(separator: "\n") + "\n", b.joined(separator: "\n") + "\n")
}

func testTimings() {
    for (n, budget) in [(10_000, 150.0), (100_000, 1000.0)] {
        let (a, b) = synthetic(n, seed: UInt64(n), change: 100)
        let t0 = DispatchTime.now().uptimeNanoseconds
        let l = TextSide.decode(Data(a.utf8))!, r = TextSide.decode(Data(b.utf8))!
        let tDecode = ms(t0)
        let t1 = DispatchTime.now().uptimeNanoseconds
        var t = TextCompare(left: l, right: r)
        let tDiff = ms(t1)
        // an edit in the middle, re-diffed in its window
        let mid = t.rows.count / 2
        let t2 = DispatchTime.now().uptimeNanoseconds
        let line = t.lineIndex(.left, atRow: mid)
        t.replace(.left, line..<(line + 1), with: ["edited line"])
        let tEdit = ms(t2)
        print(String(format: "  %dk lines: decode %.0f ms, diff %.0f ms (%d sections), edit re-diff %.1f ms",
                     n / 1000, tDecode, tDiff, t.sections.count, tEdit))
        check(tDecode + tDiff < budget, "\(n) lines decoded + diffed within \(Int(budget)) ms (\(Int(tDecode + tDiff)))")
        check(tEdit < 50, "\(n) lines: an edit re-diffed within 50 ms (\(tEdit))")
        check(consistent(t), "\(n) lines: rows consistent")
    }
}

@main
struct Main {
    static func main() {
        testRoundTrips()
        testImportance()
        testRows()
        testEdits()
        testPhase3()
        testParity()
        testTimings()
        try? fm.removeItem(atPath: tmp)
        print("\(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
