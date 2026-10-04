import Foundation

// MARK: - Compare: the text engine (Foundation only)
//
// What the Compare view's Text Compare runs on (CompareWindow.swift draws it):
//   TextSide      one side's bytes <-> lines: encoding (UTF-8 ± BOM,
//                 UTF-16 LE / BE with BOM, else Latin-1), each line's own
//                 line ending, binary sniffing. decode → encode is byte-exact.
//   Importance    the normalizer: which differences are "unimportant" (blue)
//   LineDiff      git's histogram diff (xdiff/xhistogram.c) + its group
//                 compaction and indent heuristic (xdiff/xdiffi.c), Myers
//                 (CollectionDifference) where git falls back to Myers —
//                 hunks match `git diff --no-index --histogram`
//   TextCompare   two sides + rows (filler-aligned) + sections, edits with
//                 a windowed re-diff, copy across, undo / redo
//   CharDiff      word / space / punctuation tokens within a changed line
//                 pair (the AI view's word diff lives here too)
// Tests: bin/run-tests.sh compare (Tests/test_compare.swift: git parity
// corpus, byte-exact round trips, copy + undo, timings).

enum CompareSide: String {
    case left, right
    var other: CompareSide { self == .left ? .right : .left }
}

// MARK: - TextSide

enum TextEncodingKind: String {
    case utf8 = "UTF-8", utf8BOM = "UTF-8 BOM", utf16LE = "UTF-16 LE", utf16BE = "UTF-16 BE", latin1 = "Latin-1"
}

// a line's terminator; `none` = the last line of a file without a final newline
enum EOL: UInt8 {
    case none, lf, crlf, cr
    var bytes: [UInt8] {
        switch self {
        case .none: return []
        case .lf: return [10]
        case .crlf: return [13, 10]
        case .cr: return [13]
        }
    }
    var label: String {
        switch self {
        case .none: return "none"
        case .lf: return "LF"
        case .crlf: return "CRLF"
        case .cr: return "CR"
        }
    }
}

struct TextSide {
    var lines: [String] = []
    var eols: [EOL] = []                // parallel to lines
    var encoding: TextEncodingKind = .utf8

    // NUL in the first 8 KB (no UTF-16 BOM) = binary: not shown as text
    static func isBinary(_ data: Data) -> Bool {
        if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) { return false }
        return data.prefix(8192).contains(0)
    }

    // bytes -> lines; nil only for binary data (check isBinary first)
    static func decode(_ data: Data) -> TextSide? {
        if isBinary(data) { return nil }
        var side = TextSide()
        if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) {
            let le = data.first == 0xFF
            if let s = String(data: data.dropFirst(2), encoding: le ? .utf16LittleEndian : .utf16BigEndian) {
                side.encoding = le ? .utf16LE : .utf16BE
                side.split(Array(s.utf8))
                return side
            }
        }
        var body = data
        if data.starts(with: [0xEF, 0xBB, 0xBF]) {
            body = data.dropFirst(3)
            side.encoding = .utf8BOM
        }
        if String(data: body, encoding: .utf8) != nil {
            side.split([UInt8](body))
        } else {
            // not UTF-8: Latin-1 maps every byte to one character (round trips)
            side.encoding = .latin1
            let s = String(data: data, encoding: .isoLatin1) ?? ""
            side.split(Array(s.utf8))
        }
        return side
    }

    // split UTF-8 bytes at LF / CRLF / CR (never inside a multi-byte character)
    private mutating func split(_ u: [UInt8]) {
        lines = []
        eols = []
        lines.reserveCapacity(u.count / 32)
        u.withUnsafeBufferPointer { p in
            var start = 0, i = 0
            let n = p.count
            while i < n {
                let c = p[i]
                if c == 10 || c == 13 {
                    lines.append(String(decoding: UnsafeBufferPointer(rebasing: p[start..<i]), as: UTF8.self))
                    if c == 13 && i + 1 < n && p[i + 1] == 10 {
                        eols.append(.crlf)
                        i += 1
                    } else {
                        eols.append(c == 10 ? .lf : .cr)
                    }
                    start = i + 1
                }
                i += 1
            }
            if start < n {
                lines.append(String(decoding: UnsafeBufferPointer(rebasing: p[start..<n]), as: UTF8.self))
                eols.append(.none)
            }
        }
    }

    init() {}

    // pasted text / an editor's string (UTF-8, its own line endings)
    init(text: String) {
        split(Array(text.utf8))
    }

    // the bytes to save; nil = a character the side's encoding can't hold
    // (Latin-1): the caller saves as UTF-8 instead
    func encoded() -> Data? {
        var u: [UInt8] = []
        u.reserveCapacity(lines.reduce(0) { $0 + $1.utf8.count + 2 })
        for (i, l) in lines.enumerated() {
            u.append(contentsOf: l.utf8)
            u.append(contentsOf: eols[i].bytes)
        }
        switch encoding {
        case .utf8: return Data(u)
        case .utf8BOM: return Data([0xEF, 0xBB, 0xBF] + u)
        case .utf16LE, .utf16BE:
            let s = String(decoding: u, as: UTF8.self)
            let le = encoding == .utf16LE
            guard let d = s.data(using: le ? .utf16LittleEndian : .utf16BigEndian) else { return nil }
            return Data(le ? [0xFF, 0xFE] : [0xFE, 0xFF]) + d
        case .latin1:
            return String(decoding: u, as: UTF8.self).data(using: .isoLatin1, allowLossyConversion: false)
        }
    }

    var text: String {
        var s = ""
        for (i, l) in lines.enumerated() {
            s += l
            switch eols[i] {
            case .none: break
            case .lf: s += "\n"
            case .crlf: s += "\r\n"
            case .cr: s += "\r"
            }
        }
        return s
    }

    // the most common line ending (LF when there is none yet)
    var dominantEOL: EOL {
        var n = [0, 0, 0, 0]
        for e in eols.prefix(5000) { n[Int(e.rawValue)] += 1 }
        let best = (1...3).max { n[$0] < n[$1] } ?? 1
        return n[best] == 0 ? .lf : EOL(rawValue: UInt8(best)) ?? .lf
    }

    // one side's mixed endings, for the status line ("LF", "CRLF", "mixed")
    var eolLabel: String {
        let kinds = Set(eols.filter { $0 != .none })
        if kinds.count > 1 { return "mixed" }
        return kinds.first?.label ?? dominantEOL.label
    }

    // replace lines `range` with `new` (their endings: the side's usual one,
    // and the file's last line keeps whether it had a final newline). The
    // edit that happened comes back (its range may grow by one line: text
    // appended after a last line that had no newline gives that line one)
    mutating func replace(_ range: Range<Int>, with new: [String]) -> TextEdit {
        var range = range
        var new = new
        let eol = dominantEOL
        var newEols = [EOL](repeating: eol, count: new.count)
        if range.upperBound == lines.count {
            if range.isEmpty, range.lowerBound > 0, eols[range.lowerBound - 1] == .none, !new.isEmpty {
                // appending after a last line without newline: it gets one
                range = (range.lowerBound - 1)..<range.upperBound
                new.insert(lines[range.lowerBound], at: 0)
                newEols.insert(eol, at: 0)
                newEols[newEols.count - 1] = .none
            } else if !range.isEmpty, eols[range.upperBound - 1] == .none, !new.isEmpty {
                newEols[newEols.count - 1] = .none
            }
        }
        let e = TextEdit(start: range.lowerBound, old: Array(lines[range]), oldEols: Array(eols[range]),
                         new: new, newEols: newEols)
        apply(e)
        return e
    }

    // exactly the lines + endings of an edit (undo / redo)
    mutating func apply(_ e: TextEdit, reverse: Bool = false) {
        let r = e.start..<(e.start + (reverse ? e.new.count : e.old.count))
        lines.replaceSubrange(r, with: reverse ? e.old : e.new)
        eols.replaceSubrange(r, with: reverse ? e.oldEols : e.newEols)
    }
}

// one replacement of a run of lines (one undo step)
struct TextEdit {
    var start: Int
    var old: [String]
    var oldEols: [EOL]
    var new: [String]
    var newEols: [EOL]
}

// MARK: - Importance

// Which differences don't matter (drawn blue, "unimportant"). The shown text
// never changes: lines are compared by their `key`.
struct Importance: Equatable {
    var leadingWS = true
    var trailingWS = true
    var embeddedWS = false
    var ignoreCase = false
    var lineEndings = true      // CRLF vs LF (and a missing final newline) ignored
    var blankLines = false      // inserted / deleted blank lines are unimportant

    // everything counts (git's view of a file)
    static let exact = Importance(leadingWS: false, trailingWS: false, embeddedWS: false,
                                  ignoreCase: false, lineEndings: false, blankLines: false)

    private static func ws(_ c: UInt8) -> Bool { c == 32 || c == 9 || c == 11 || c == 12 }

    // the comparison key of a line (its line ending rides along unless ignored)
    func key(_ s: String, _ eol: EOL) -> String {
        var k = normalized(s)
        if !lineEndings { k.unicodeScalars.append(Unicode.Scalar(0xE000 + UInt32(eol.rawValue))!) }
        return k
    }

    func normalized(_ s: String) -> String {
        if !leadingWS && !trailingWS && !embeddedWS && !ignoreCase { return s }
        let u = s.utf8
        guard !u.isEmpty else { return s }
        var out: String
        if !embeddedWS {
            if (!leadingWS || !Self.ws(u.first!)) && (!trailingWS || !Self.ws(u.last!)) {
                out = s
            } else {
                var lo = u.startIndex, hi = u.endIndex
                if leadingWS { while lo < hi, Self.ws(u[lo]) { lo = u.index(after: lo) } }
                if trailingWS { while hi > lo, Self.ws(u[u.index(before: hi)]) { hi = u.index(before: hi) } }
                out = String(s[lo..<hi])
            }
        } else {
            // leading / trailing per their switches, every space inside dropped
            let b = Array(u)
            var lo = 0, hi = b.count
            var first = 0, last = b.count
            while first < b.count, Self.ws(b[first]) { first += 1 }
            while last > first, Self.ws(b[last - 1]) { last -= 1 }
            if leadingWS { lo = first }
            if trailingWS { hi = max(last, lo) }
            var o: [UInt8] = []
            o.reserveCapacity(hi - lo)
            for i in lo..<hi where !(i >= first && i < last && Self.ws(b[i])) { o.append(b[i]) }
            out = String(decoding: o, as: UTF8.self)
        }
        return ignoreCase ? out.lowercased() : out
    }

    func isBlank(_ s: String) -> Bool { s.utf8.allSatisfy(Self.ws) }
}

// MARK: - LineDiff (git histogram)

enum LineDiff {
    // a change group: lines a..<a+n on the left replaced by b..<b+m on the right
    struct Hunk: Equatable {
        var a, n, b, m: Int
    }

    // hunks between two lists of line ids (equal id = equal line). `textA` /
    // `textB` feed the indent heuristic (git's default)
    static func hunks(_ A: [Int32], _ B: [Int32], textA: [String], textB: [String]) -> [Hunk] {
        var ca = [Bool](repeating: false, count: A.count)
        var cb = [Bool](repeating: false, count: B.count)
        histogram(A, B, &ca, &cb)
        compact(&ca, cb, A, textA)
        compact(&cb, ca, B, textB)
        return build(ca, cb)
    }

    static func build(_ ca: [Bool], _ cb: [Bool]) -> [Hunk] {
        var out: [Hunk] = []
        var i = 0, j = 0
        let n = ca.count, m = cb.count
        while i < n || j < m {
            if (i < n && ca[i]) || (j < m && cb[j]) {
                let a = i, b = j
                while i < n && ca[i] { i += 1 }
                while j < m && cb[j] { j += 1 }
                out.append(Hunk(a: a, n: i - a, b: b, m: j - b))
            } else {
                i += 1
                j += 1
            }
        }
        return out
    }

    // xhistogram.c: common ends trimmed (xdl_trim_ends), then regions split
    // at their longest common run of rarest lines; a region whose common
    // lines all repeat more than 64 times falls back to Myers. A work list
    // instead of recursion (deep files on a 512 KB thread stack).
    static func histogram(_ A: [Int32], _ B: [Int32], _ ca: inout [Bool], _ cb: inout [Bool]) {
        let n = A.count, m = B.count
        var pre = 0
        while pre < n && pre < m && A[pre] == B[pre] { pre += 1 }
        var suf = 0
        while suf < n - pre && suf < m - pre && A[n - 1 - suf] == B[m - 1 - suf] { suf += 1 }
        var work: [(Int, Int, Int, Int)] = [(pre, n - suf - pre, pre, m - suf - pre)]
        while let (l1, c1, l2, c2) = work.popLast() {
            if c1 <= 0 && c2 <= 0 { continue }
            if c1 <= 0 { for k in l2..<(l2 + c2) { cb[k] = true }; continue }
            if c2 <= 0 { for k in l1..<(l1 + c1) { ca[k] = true }; continue }
            switch findLCS(A, B, l1, c1, l2, c2) {
            case .fallback:
                let d = Array(B[l2..<(l2 + c2)]).difference(from: Array(A[l1..<(l1 + c1)]))
                for ch in d {
                    switch ch {
                    case .remove(let o, _, _): ca[l1 + o] = true
                    case .insert(let o, _, _): cb[l2 + o] = true
                    }
                }
            case .none:
                for k in l1..<(l1 + c1) { ca[k] = true }
                for k in l2..<(l2 + c2) { cb[k] = true }
            case .lcs(let b1, let e1, let b2, let e2):
                work.append((e1 + 1, l1 + c1 - 1 - e1, e2 + 1, l2 + c2 - 1 - e2))
                work.append((l1, b1 - l1, l2, b2 - l2))
            }
        }
    }

    private enum LCS { case none, fallback, lcs(Int, Int, Int, Int) }

    private static let maxChain = 64

    private static func findLCS(_ A: [Int32], _ B: [Int32], _ l1: Int, _ c1: Int, _ l2: Int, _ c2: Int) -> LCS {
        let end1 = l1 + c1 - 1, end2 = l2 + c2 - 1
        // scanA: one record per distinct line (first occurrence + count), a
        // chain of later occurrences, each line's record
        var recOf = [Int32: Int](minimumCapacity: c1)
        var recPtr: [Int] = [], recCnt: [Int] = []
        var next = [Int](repeating: -1, count: c1)
        var lineRec = [Int](repeating: 0, count: c1)
        var ptr = end1
        while ptr >= l1 {
            let id = A[ptr]
            if let r = recOf[id] {
                next[ptr - l1] = recPtr[r]
                recPtr[r] = ptr
                recCnt[r] = min(Int(Int32.max), recCnt[r] + 1)
                lineRec[ptr - l1] = r
            } else {
                let r = recPtr.count
                recPtr.append(ptr)
                recCnt.append(1)
                recOf[id] = r
                lineRec[ptr - l1] = r
            }
            ptr -= 1
        }
        var best = maxChain + 1
        var hasCommon = false
        var found = false
        var lb1 = 0, le1 = 0, lb2 = 0, le2 = 0
        var bptr = l2
        while bptr <= end2 {
            var bnext = bptr + 1
            if let r = recOf[B[bptr]] {
                if recCnt[r] > best {
                    hasCommon = true
                } else {
                    hasCommon = true
                    var as_ = recPtr[r]
                    while true {
                        var np = next[as_ - l1]
                        var bs = bptr, ae = as_, be = bptr
                        var rc = recCnt[r]
                        while l1 < as_ && l2 < bs && A[as_ - 1] == B[bs - 1] {
                            as_ -= 1
                            bs -= 1
                            if 1 < rc { rc = min(rc, recCnt[lineRec[as_ - l1]]) }
                        }
                        while ae < end1 && be < end2 && A[ae + 1] == B[be + 1] {
                            ae += 1
                            be += 1
                            if 1 < rc { rc = min(rc, recCnt[lineRec[ae - l1]]) }
                        }
                        if bnext <= be { bnext = be + 1 }
                        if le1 - lb1 < ae - as_ || rc < best {
                            lb1 = as_; le1 = ae; lb2 = bs; le2 = be
                            best = rc
                            found = true
                        }
                        if np < 0 { break }
                        var stop = false
                        while np <= ae {
                            np = next[np - l1]
                            if np < 0 { stop = true; break }
                        }
                        if stop { break }
                        as_ = np
                    }
                }
            }
            bptr = bnext
        }
        if hasCommon && maxChain < best { return .fallback }
        return found ? .lcs(lb1, le1, lb2, le2) : .none
    }

    // xdl_change_compact: slide every change group up / down as far as
    // equal lines allow (merging groups it bumps into), line it up with a
    // change on the other side, else place it by git's indent heuristic.
    // `ch` = this side's changed lines, `other` = the other side's (read only)
    static func compact(_ ch: inout [Bool], _ other: [Bool], _ ids: [Int32], _ text: [String]) {
        let n = ids.count, no = other.count
        // sentinels: r[0] = line -1, r[n+1] = line n — both unchanged
        var r = [Bool](repeating: false, count: n + 2)
        for i in 0..<n { r[i + 1] = ch[i] }
        var ro = [Bool](repeating: false, count: no + 2)
        for i in 0..<no { ro[i + 1] = other[i] }
        var indents = [Int16](repeating: -2, count: n)
        func indent(_ i: Int) -> Int {
            if indents[i] != -2 { return Int(indents[i]) }
            var ret = 0, v = -1
            for c in text[i].utf8 {
                if c == 32 { ret += 1 } else if c == 9 { ret += 8 - ret % 8 }
                else if c == 11 || c == 12 || c == 13 || c == 10 {} else { v = ret; break }
                if ret >= 200 { v = 200; break }
            }
            indents[i] = Int16(v)
            return v
        }
        var gs = 0, ge = 0          // this side's group [gs, ge)
        var os = 0, oe = 0          // the other side's
        while r[ge + 1] { ge += 1 }
        while ro[oe + 1] { oe += 1 }
        func next(_ s: inout Int, _ e: inout Int, _ rr: [Bool], _ cnt: Int) -> Bool {
            if e == cnt { return false }
            s = e + 1
            e = s
            while rr[e + 1] { e += 1 }
            return true
        }
        func prev(_ s: inout Int, _ e: inout Int, _ rr: [Bool]) -> Bool {
            if s == 0 { return false }
            e = s - 1
            s = e
            while rr[s] { s -= 1 }      // rr[s] = line s - 1
            return true
        }
        func slideDown() -> Bool {
            guard ge < n, ids[gs] == ids[ge] else { return false }
            r[gs + 1] = false
            gs += 1
            r[ge + 1] = true
            ge += 1
            while r[ge + 1] { ge += 1 }
            return true
        }
        func slideUp() -> Bool {
            guard gs > 0, ids[gs - 1] == ids[ge - 1] else { return false }
            gs -= 1
            r[gs + 1] = true
            ge -= 1
            r[ge + 1] = false
            while r[gs] { gs -= 1 }
            return true
        }
        struct Measure { var eof = false, indent = -1, preBlank = 0, preIndent = -1, postBlank = 0, postIndent = -1 }
        func measure(_ split: Int) -> Measure {
            var m = Measure()
            if split >= n { m.eof = true; m.indent = -1 } else { m.indent = indent(split) }
            var i = split - 1
            while i >= 0 {
                m.preIndent = indent(i)
                if m.preIndent != -1 { break }
                m.preBlank += 1
                if m.preBlank == 20 { m.preIndent = 0; break }
                i -= 1
            }
            i = split + 1
            while i < n {
                m.postIndent = indent(i)
                if m.postIndent != -1 { break }
                m.postBlank += 1
                if m.postBlank == 20 { m.postIndent = 0; break }
                i += 1
            }
            return m
        }
        func score(_ m: Measure, _ s: inout (indent: Int, penalty: Int)) {
            if m.preIndent == -1 && m.preBlank == 0 { s.penalty += 1 }
            if m.eof { s.penalty += 21 }
            let postBlank = m.indent == -1 ? 1 + m.postBlank : 0
            let totalBlank = m.preBlank + postBlank
            s.penalty += -30 * totalBlank
            s.penalty += 6 * postBlank
            let ind = m.indent != -1 ? m.indent : m.postIndent
            let any = totalBlank != 0
            s.indent += ind
            if ind == -1 || m.preIndent == -1 {
            } else if ind > m.preIndent {
                s.penalty += any ? 10 : -4
            } else if ind == m.preIndent {
            } else if m.postIndent != -1 && m.postIndent > ind {
                s.penalty += any ? 17 : 24
            } else {
                s.penalty += any ? 17 : 23
            }
        }
        func cmp(_ a: (indent: Int, penalty: Int), _ b: (indent: Int, penalty: Int)) -> Int {
            let ci = (a.indent > b.indent ? 1 : 0) - (a.indent < b.indent ? 1 : 0)
            return 60 * ci + (a.penalty - b.penalty)
        }
        while true {
            if ge != gs {
                var size = 0, earliest = 0, matching = -1
                repeat {
                    size = ge - gs
                    matching = -1
                    while slideUp() { _ = prev(&os, &oe, ro) }
                    earliest = ge
                    if oe > os { matching = ge }
                    while slideDown() {
                        _ = next(&os, &oe, ro, no)
                        if oe > os { matching = ge }
                    }
                } while size != ge - gs
                if ge == earliest {
                } else if matching != -1 {
                    while oe == os {
                        _ = slideUp()
                        _ = prev(&os, &oe, ro)
                    }
                } else {
                    var shift = earliest
                    if ge - size - 1 > shift { shift = ge - size - 1 }
                    if ge - 100 > shift { shift = ge - 100 }
                    var bestShift = -1
                    var bestScore = (indent: 0, penalty: 0)
                    while shift <= ge {
                        var s = (indent: 0, penalty: 0)
                        score(measure(shift), &s)
                        score(measure(shift - size), &s)
                        if bestShift == -1 || cmp(s, bestScore) <= 0 {
                            bestScore = s
                            bestShift = shift
                        }
                        shift += 1
                    }
                    while ge > bestShift {
                        _ = slideUp()
                        _ = prev(&os, &oe, ro)
                    }
                }
            }
            if !next(&gs, &ge, r, n) { break }
            _ = next(&os, &oe, ro, no)
        }
        for i in 0..<n { ch[i] = r[i + 1] }
    }
}

// MARK: - rows + sections

enum RowKind: UInt8 { case same, changed, leftOnly, rightOnly }

// one display row: a left line and / or a right line (-1 = filler)
struct CompareRow: Equatable {
    var l: Int32
    var r: Int32
    var kind: RowKind
    var important: Bool         // false = only unimportant differences (blue)
    func line(_ s: CompareSide) -> Int { Int(s == .left ? l : r) }
}

struct CompareSection: Equatable {
    var rows: Range<Int>
    var important: Bool
}

enum CompareFilter: String, CaseIterable {
    case all, diffs, same, context
    var title: String {
        switch self {
        case .all: return "All"
        case .diffs: return "Diffs"
        case .same: return "Same"
        case .context: return "Context"
        }
    }
}

// MARK: - TextCompare (one Text Compare session's model)

struct TextCompare {
    var left = TextSide()
    var right = TextSide()
    var importance = Importance()
    var ignoreUnimportant = false
    private(set) var rows: [CompareRow] = []
    private(set) var sections: [CompareSection] = []
    // interned comparison keys (equal id = equal key)
    private var keysL: [Int32] = []
    private var keysR: [Int32] = []
    private var intern: [String: Int32] = [:]
    // undo / redo: (side, edit)
    private(set) var undoStack: [(CompareSide, TextEdit)] = []
    private(set) var redoStack: [(CompareSide, TextEdit)] = []
    static let undoLimit = 500
    // the context the windowed re-diff keeps around an edit
    static let rediffContext = 50

    init() {}

    init(left: TextSide, right: TextSide, importance: Importance = Importance(), ignoreUnimportant: Bool = false) {
        self.left = left
        self.right = right
        self.importance = importance
        self.ignoreUnimportant = ignoreUnimportant
        recompute()
    }

    func side(_ s: CompareSide) -> TextSide { s == .left ? left : right }

    private mutating func id(_ k: String) -> Int32 {
        if let i = intern[k] { return i }
        let i = Int32(intern.count)
        intern[k] = i
        return i
    }

    private mutating func keys(_ t: TextSide, _ range: Range<Int>) -> [Int32] {
        var out: [Int32] = []
        out.reserveCapacity(range.count)
        for i in range { out.append(id(importance.key(t.lines[i], t.eols[i]))) }
        return out
    }

    // the full diff (open, reload, importance change)
    mutating func recompute() {
        intern = [:]
        intern.reserveCapacity(left.lines.count + right.lines.count)
        keysL = keys(left, 0..<left.lines.count)
        keysR = keys(right, 0..<right.lines.count)
        rows = buildRows(0..<left.lines.count, 0..<right.lines.count)
        computeSections()
    }

    // rows for left lines `la` against right lines `ra` (absolute indices)
    private func buildRows(_ la: Range<Int>, _ ra: Range<Int>) -> [CompareRow] {
        let A = Array(keysL[la]), B = Array(keysR[ra])
        let hs = LineDiff.hunks(A, B, textA: Array(left.lines[la]), textB: Array(right.lines[ra]))
        var out: [CompareRow] = []
        out.reserveCapacity(max(A.count, B.count) + 16)
        var i = 0, j = 0
        func matched(_ i: Int, _ j: Int) {
            let li = la.lowerBound + i, rj = ra.lowerBound + j
            let same = rawEqual(li, rj)
            out.append(CompareRow(l: Int32(li), r: Int32(rj), kind: same ? .same : .changed, important: false))
        }
        for h in hs {
            while i < h.a { matched(i, j); i += 1; j += 1 }
            pairRows(h, la.lowerBound, ra.lowerBound, &out)
            i = h.a + h.n
            j = h.b + h.m
        }
        while i < A.count { matched(i, j); i += 1; j += 1 }
        return out
    }

    private func rawEqual(_ li: Int, _ rj: Int) -> Bool {
        left.lines[li].utf8.elementsEqual(right.lines[rj].utf8)
            && (importance.lineEndings || left.eols[li] == right.eols[rj])
    }

    // a hunk's rows: lines similar enough are lined up (the DP below picks
    // where the fillers go when the counts differ); the rest side by side
    private func pairRows(_ h: LineDiff.Hunk, _ lo: Int, _ ro: Int, _ out: inout [CompareRow]) {
        let a0 = lo + h.a, b0 = ro + h.b
        func row(_ l: Int?, _ r: Int?) {
            if let l, let r {
                if rawEqual(l, r) {
                    out.append(CompareRow(l: Int32(l), r: Int32(r), kind: .same, important: false))
                } else {
                    out.append(CompareRow(l: Int32(l), r: Int32(r), kind: .changed,
                                          important: keysL[l] != keysR[r]))
                }
            } else if let l {
                let imp = !(importance.blankLines && importance.isBlank(left.lines[l]))
                out.append(CompareRow(l: Int32(l), r: -1, kind: .leftOnly, important: imp))
            } else if let r {
                let imp = !(importance.blankLines && importance.isBlank(right.lines[r]))
                out.append(CompareRow(l: -1, r: Int32(r), kind: .rightOnly, important: imp))
            }
        }
        func zip(_ a: Range<Int>, _ b: Range<Int>) {
            let n = max(a.count, b.count)
            for k in 0..<n {
                row(k < a.count ? a.lowerBound + k : nil, k < b.count ? b.lowerBound + k : nil)
            }
        }
        if h.n == 0 || h.m == 0 || h.n == h.m || h.n * h.m > 4096 {
            zip(a0..<(a0 + h.n), b0..<(b0 + h.m))
            return
        }
        // anchors: the monotone pairing with the most similarity (≥ 0.5 each)
        let n = h.n, m = h.m
        let ga = (0..<n).map { Self.bigrams(importance.normalized(left.lines[a0 + $0])) }
        let gb = (0..<m).map { Self.bigrams(importance.normalized(right.lines[b0 + $0])) }
        var sim = [Double](repeating: 0, count: n * m)
        for x in 0..<n { for y in 0..<m { sim[x * m + y] = Self.dice(ga[x], gb[y]) } }
        var dp = [Double](repeating: 0, count: (n + 1) * (m + 1))
        let w = m + 1
        for x in 1...n {
            for y in 1...m {
                var v = max(dp[(x - 1) * w + y], dp[x * w + y - 1])
                let s = sim[(x - 1) * m + y - 1]
                if s >= 0.5 { v = max(v, dp[(x - 1) * w + y - 1] + s) }
                dp[x * w + y] = v
            }
        }
        var pairs: [(Int, Int)] = []
        var x = n, y = m
        while x > 0 && y > 0 {
            let s = sim[(x - 1) * m + y - 1]
            if s >= 0.5 && dp[x * w + y] == dp[(x - 1) * w + y - 1] + s {
                pairs.append((x - 1, y - 1)); x -= 1; y -= 1
            } else if dp[x * w + y] == dp[(x - 1) * w + y] {
                x -= 1
            } else {
                y -= 1
            }
        }
        pairs.reverse()
        var pa = 0, pb = 0
        for (px, py) in pairs {
            zip((a0 + pa)..<(a0 + px), (b0 + pb)..<(b0 + py))
            row(a0 + px, b0 + py)
            pa = px + 1
            pb = py + 1
        }
        zip((a0 + pa)..<(a0 + n), (b0 + pb)..<(b0 + m))
    }

    private static func bigrams(_ s: String) -> [UInt32: Int] {
        var d: [UInt32: Int] = [:]
        var prev: UInt16?
        for c in s.utf16 {
            if let p = prev { d[UInt32(p) << 16 | UInt32(c), default: 0] += 1 }
            prev = c
        }
        return d
    }

    private static func dice(_ a: [UInt32: Int], _ b: [UInt32: Int]) -> Double {
        let ta = a.values.reduce(0, +), tb = b.values.reduce(0, +)
        guard ta + tb > 0 else { return 1 }
        var common = 0
        for (k, v) in a { if let w = b[k] { common += min(v, w) } }
        return 2 * Double(common) / Double(ta + tb)
    }

    // a row that counts as a difference (with Ignore Unimportant on, blue
    // rows count as the same)
    func isDiff(_ r: CompareRow) -> Bool {
        r.kind != .same && (r.important || !ignoreUnimportant)
    }

    private mutating func computeSections() {
        var out: [CompareSection] = []
        var i = 0
        let n = rows.count
        while i < n {
            if isDiff(rows[i]) {
                let s = i
                var imp = false
                while i < n && isDiff(rows[i]) { imp = imp || rows[i].important; i += 1 }
                out.append(CompareSection(rows: s..<i, important: imp))
            } else {
                i += 1
            }
        }
        sections = out
    }

    mutating func setIgnoreUnimportant(_ on: Bool) {
        ignoreUnimportant = on
        computeSections()
    }

    var importantCount: Int { sections.filter(\.important).count }
    var unimportantCount: Int { sections.count - importantCount }
    var identicalText: Bool { rows.allSatisfy { $0.kind == .same } }

    // the section holding row `row` (nil = a same row)
    func section(at row: Int) -> Int? {
        var lo = 0, hi = sections.count - 1
        while lo <= hi {
            let mid = (lo + hi) / 2
            let s = sections[mid].rows
            if row < s.lowerBound { hi = mid - 1 } else if row >= s.upperBound { lo = mid + 1 } else { return mid }
        }
        return nil
    }

    // the next / previous section after / before row `row` (no wrap)
    func nextSection(after row: Int) -> Int? { sections.firstIndex { $0.rows.lowerBound > row } }
    func prevSection(before row: Int) -> Int? { sections.lastIndex { $0.rows.lowerBound < row } }

    // lines of `side` before row `row` (an insertion point at a filler)
    func lineIndex(_ side: CompareSide, atRow row: Int) -> Int {
        var i = row
        while i < rows.count {
            let v = rows[i].line(side)
            if v >= 0 { return v }
            i += 1
        }
        return side == .left ? left.lines.count : right.lines.count
    }

    // the lines of `side` inside rows `range` (contiguous), or the insertion
    // point when the side has only fillers there
    func lineRange(_ side: CompareSide, rows range: Range<Int>) -> Range<Int> {
        let start = lineIndex(side, atRow: range.lowerBound)
        let end = range.upperBound >= rows.count ? (side == .left ? left.lines.count : right.lines.count)
            : lineIndex(side, atRow: range.upperBound)
        return start..<max(start, end)
    }

    // the rows a display filter shows (nil = every row)
    func visibleRows(_ f: CompareFilter, context: Int) -> [Int]? {
        switch f {
        case .all: return nil
        case .diffs: return rows.indices.filter { isDiff(rows[$0]) }
        case .same: return rows.indices.filter { !isDiff(rows[$0]) }
        case .context:
            var keep = [Bool](repeating: false, count: rows.count)
            for s in sections {
                let lo = max(0, s.rows.lowerBound - context), hi = min(rows.count, s.rows.upperBound + context)
                for i in lo..<hi { keep[i] = true }
            }
            return rows.indices.filter { keep[$0] }
        }
    }

    // MARK: edits

    // replace `side`'s lines `range` with `new`; one undo step. Re-diffs
    // only around the edit (± rediffContext unchanged rows)
    @discardableResult
    mutating func replace(_ side: CompareSide, _ range: Range<Int>, with new: [String], undoable: Bool = true) -> TextEdit {
        let e: TextEdit
        if side == .left { e = left.replace(range, with: new) } else { e = right.replace(range, with: new) }
        applied(side, e)
        if undoable {
            undoStack.append((side, e))
            if undoStack.count > Self.undoLimit { undoStack.removeFirst() }
            redoStack = []
        }
        return e
    }

    // an edit already in the side's lines: keys + rows follow
    private mutating func applied(_ side: CompareSide, _ e: TextEdit, reverse: Bool = false) {
        let oldCount = reverse ? e.new.count : e.old.count
        let newLines = reverse ? e.old : e.new
        let newEols = reverse ? e.oldEols : e.newEols
        var ks: [Int32] = []
        for (i, l) in newLines.enumerated() { ks.append(id(importance.key(l, newEols[i]))) }
        let r = e.start..<(e.start + oldCount)
        if side == .left { keysL.replaceSubrange(r, with: ks) } else { keysR.replaceSubrange(r, with: ks) }
        rediff(side, start: e.start, oldCount: oldCount, newCount: newLines.count)
    }

    // the windowed re-diff: rows from the last `rediffContext` unchanged
    // rows before the edit to as many after it are diffed again; the rest
    // only shift their line numbers
    private mutating func rediff(_ side: CompareSide, start: Int, oldCount: Int, newCount: Int) {
        let delta = newCount - oldCount
        // first row at / after the edit on `side`
        var rs = rows.count
        for (i, r) in rows.enumerated() where r.line(side) >= start { rs = i; break }
        var re = rs
        if oldCount > 0 {
            let last = start + oldCount - 1
            re = rs
            while re < rows.count && (rows[re].line(side) < 0 || rows[re].line(side) <= last) { re += 1 }
        }
        // grow to `rediffContext` same rows on each side (or the ends)
        var ws = rs, seen = 0
        while ws > 0 {
            if rows[ws - 1].kind == .same {
                seen += 1
                if seen > Self.rediffContext { break }
            }
            ws -= 1
        }
        var we = re
        seen = 0
        while we < rows.count {
            if rows[we].kind == .same {
                seen += 1
                if seen > Self.rediffContext { break }
            }
            we += 1
        }
        // a window boundary must sit between two same rows' lines on both sides
        func lineAt(_ s: CompareSide, _ row: Int, oldCount total: Int) -> Int {
            var i = row
            while i < rows.count {
                let v = rows[i].line(s)
                if v >= 0 { return v }
                i += 1
            }
            return total
        }
        let oldLeftTotal = left.lines.count - (side == .left ? delta : 0)
        let oldRightTotal = right.lines.count - (side == .right ? delta : 0)
        let l0 = lineAt(.left, ws, oldCount: oldLeftTotal), l1 = lineAt(.left, we, oldCount: oldLeftTotal)
        let r0 = lineAt(.right, ws, oldCount: oldRightTotal), r1 = lineAt(.right, we, oldCount: oldRightTotal)
        let la = l0..<(l1 + (side == .left ? delta : 0))
        let ra = r0..<(r1 + (side == .right ? delta : 0))
        let fresh = buildRows(la, ra)
        if delta != 0 {
            for i in we..<rows.count {
                if side == .left, rows[i].l >= 0 { rows[i].l += Int32(delta) }
                if side == .right, rows[i].r >= 0 { rows[i].r += Int32(delta) }
            }
        }
        rows.replaceSubrange(ws..<we, with: fresh)
        computeSections()
    }

    // copy rows `range` from `from` to the other side (a section, a row
    // selection, one line): the other side's lines there become these
    @discardableResult
    mutating func copyRows(_ range: Range<Int>, from: CompareSide) -> TextEdit? {
        guard !range.isEmpty, range.upperBound <= rows.count else { return nil }
        let src = lineRange(from, rows: range)
        let dst = lineRange(from.other, rows: range)
        let lines = Array(side(from).lines[src])
        if Array(side(from.other).lines[dst]) == lines { return nil }
        return replace(from.other, dst, with: lines)
    }

    @discardableResult
    mutating func copySection(_ i: Int, from: CompareSide) -> TextEdit? {
        guard sections.indices.contains(i) else { return nil }
        return copyRows(sections[i].rows, from: from)
    }

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }
    func lastEditSide() -> CompareSide? { undoStack.last?.0 }

    // undo the last edit (of `side` when given and it has one, else the last)
    @discardableResult
    mutating func undo(_ side: CompareSide? = nil) -> TextEdit? {
        var idx = undoStack.count - 1
        if let side, let i = undoStack.lastIndex(where: { $0.0 == side }) { idx = i }
        guard idx >= 0 else { return nil }
        // only the newest edit of a side can be taken back safely: later
        // edits of the OTHER side don't move this side's lines
        let (s, e) = undoStack.remove(at: idx)
        if s == .left { left.apply(e, reverse: true) } else { right.apply(e, reverse: true) }
        applied(s, e, reverse: true)
        redoStack.append((s, e))
        return e
    }

    @discardableResult
    mutating func redo() -> TextEdit? {
        guard let (s, e) = redoStack.popLast() else { return nil }
        if s == .left { left.apply(e) } else { right.apply(e) }
        applied(s, e)
        undoStack.append((s, e))
        return e
    }

    mutating func setImportance(_ imp: Importance) {
        importance = imp
        recompute()
    }

    mutating func swapSides() {
        swap(&left, &right)
        undoStack = undoStack.map { ($0.0.other, $0.1) }
        redoStack = redoStack.map { ($0.0.other, $0.1) }
        recompute()
    }

    // a side's text replaced wholesale (reload, paste, open a file)
    mutating func setSide(_ s: CompareSide, _ t: TextSide) {
        if s == .left { left = t } else { right = t }
        undoStack.removeAll { $0.0 == s }
        redoStack.removeAll { $0.0 == s }
        recompute()
    }
}

// MARK: - binary compare

enum BinaryCompare {
    // nil = identical; else the first byte offset that differs
    static func firstDifference(_ a: Data, _ b: Data) -> Int? {
        let n = min(a.count, b.count)
        return a.withUnsafeBytes { pa in
            b.withUnsafeBytes { pb in
                let x = pa.bindMemory(to: UInt8.self), y = pb.bindMemory(to: UInt8.self)
                var i = 0
                // 8 bytes at a time, then the tail
                while i + 8 <= n {
                    if pa.loadUnaligned(fromByteOffset: i, as: UInt64.self) != pb.loadUnaligned(fromByteOffset: i, as: UInt64.self) { break }
                    i += 8
                }
                while i < n {
                    if x[i] != y[i] { return i }
                    i += 1
                }
                return a.count == b.count ? nil : n
            }
        }
    }
}

// MARK: - CharDiff (word-level marks; also the AI view's diff)

enum CharDiff {
    enum Kind { case same, del, ins }
    struct Op { var kind: Kind; var text: String }

    // words (letters / digits, inner ' and ’ kept: they're), runs of
    // whitespace, and every other character on its own
    static func tokens(_ s: String) -> [String] {
        var out: [String] = []
        var cur = ""
        var curKind = 0     // 1 word, 2 space
        let chars = Array(s)
        func flush() { if !cur.isEmpty { out.append(cur); cur = "" }; curKind = 0 }
        for (i, ch) in chars.enumerated() {
            let isWord = ch.isLetter || ch.isNumber
                || ((ch == "'" || ch == "’") && curKind == 1 && i + 1 < chars.count && chars[i + 1].isLetter)
            let k = isWord ? 1 : ch.isWhitespace ? 2 : 3
            if k == 3 { flush(); out.append(String(ch)); continue }
            if k != curKind { flush(); curKind = k }
            cur.append(ch)
        }
        flush()
        return out
    }

    // lines incl. their newline (the fallback for very long texts)
    static func lines(_ s: String) -> [String] {
        var out: [String] = []
        var cur = ""
        for ch in s { cur.append(ch); if ch == "\n" { out.append(cur); cur = "" } }
        if !cur.isEmpty { out.append(cur) }
        return out
    }

    // token ops in order (no grouping): LCS over tokens (lines when the
    // table would be huge), common prefix / suffix first
    static func rawOps(_ a: String, _ b: String) -> [Op] {
        var x = tokens(a), y = tokens(b)
        if x.count * y.count > 6_000_000 { x = lines(a); y = lines(b) }
        let n = x.count, m = y.count
        var pre = 0
        while pre < n && pre < m && x[pre] == y[pre] { pre += 1 }
        var suf = 0
        while suf < n - pre && suf < m - pre && x[n - 1 - suf] == y[m - 1 - suf] { suf += 1 }
        let xs = Array(x[pre..<(n - suf)]), ys = Array(y[pre..<(m - suf)])
        var raw: [Op] = x[..<pre].map { Op(kind: .same, text: $0) }
        let r = xs.count, c = ys.count
        if r > 0 || c > 0 {
            // L[i][j] = LCS of xs[i...] and ys[j...]
            let w = c + 1
            var L = [Int32](repeating: 0, count: (r + 1) * w)
            if r > 0 && c > 0 {
                for i in stride(from: r - 1, through: 0, by: -1) {
                    for j in stride(from: c - 1, through: 0, by: -1) {
                        L[i * w + j] = xs[i] == ys[j] ? L[(i + 1) * w + j + 1] + 1
                            : max(L[(i + 1) * w + j], L[i * w + j + 1])
                    }
                }
            }
            var i = 0, j = 0
            while i < r || j < c {
                if i < r && j < c && xs[i] == ys[j] {
                    raw.append(Op(kind: .same, text: xs[i])); i += 1; j += 1
                } else if j < c && (i == r || L[i * w + j + 1] >= L[(i + 1) * w + j]) {
                    raw.append(Op(kind: .ins, text: ys[j])); j += 1
                } else {
                    raw.append(Op(kind: .del, text: xs[i])); i += 1
                }
            }
        }
        raw += x[(n - suf)...].map { Op(kind: .same, text: $0) }
        return raw
    }

    // the AI view's diff: each run of changes = its deletions, then its
    // insertions; spacing-only changes are not marked
    static func diff(_ a: String, _ b: String) -> [Op] {
        group(rawOps(a, b))
    }

    // a lone space between two changes joins them ("their going" ->
    // "They're gone" reads as one replacement, not two), then each change
    // run = its deletions, then its insertions
    private static func group(_ raw: [Op]) -> [Op] {
        var ops = raw
        var k = 1
        while k < ops.count - 1 {
            if ops[k].kind == .same, ops[k].text.allSatisfy({ $0 == " " }),
               ops[k - 1].kind != .same, ops[k + 1].kind != .same {
                let t = ops[k].text
                ops.replaceSubrange(k...k, with: [Op(kind: .del, text: t), Op(kind: .ins, text: t)])
                k += 2
            } else { k += 1 }
        }
        var out: [Op] = []
        var del = "", ins = ""
        func flush() {
            if del.allSatisfy(\.isWhitespace) && ins.allSatisfy(\.isWhitespace) {
                // spacing only (a table re-padded): not a change worth marking
                if !ins.isEmpty {
                    if let last = out.last, last.kind == .same { out[out.count - 1].text += ins }
                    else { out.append(Op(kind: .same, text: ins)) }
                }
            } else {
                if !del.isEmpty { out.append(Op(kind: .del, text: del)) }
                if !ins.isEmpty { out.append(Op(kind: .ins, text: ins)) }
            }
            del = ""; ins = ""
        }
        for o in ops {
            switch o.kind {
            case .del: del += o.text
            case .ins: ins += o.text
            case .same:
                flush()
                if let last = out.last, last.kind == .same { out[out.count - 1].text += o.text }
                else { out.append(o) }
            }
        }
        flush()
        return out
    }

    // how many separate changes (a replacement counts once)
    static func changes(_ ops: [Op]) -> Int {
        var n = 0
        var inChange = false
        for o in ops {
            if o.kind == .same { inChange = false } else if !inChange { n += 1; inChange = true }
        }
        return n
    }

    // a marked span of one line (UTF-16 offsets, for drawing)
    struct Mark: Equatable {
        var range: NSRange
        var important: Bool
    }

    // the changed spans of a line pair: each run of changed tokens, marked
    // unimportant when both sides' runs are equal under `imp` (spacing,
    // case)
    static func marks(_ a: String, _ b: String, _ imp: Importance) -> (left: [Mark], right: [Mark]) {
        var left: [Mark] = [], right: [Mark] = []
        var pa = 0, pb = 0
        var delStart = 0, insStart = 0, del = "", ins = ""
        func flush() {
            guard !del.isEmpty || !ins.isEmpty else { return }
            let ws = del.allSatisfy(\.isWhitespace) && ins.allSatisfy(\.isWhitespace)
            let unimportant = (ws && (imp.embeddedWS || imp.leadingWS || imp.trailingWS))
                || (!del.isEmpty && !ins.isEmpty && imp.normalized(del) == imp.normalized(ins))
            if !del.isEmpty { left.append(Mark(range: NSRange(location: delStart, length: del.utf16.count), important: !unimportant)) }
            if !ins.isEmpty { right.append(Mark(range: NSRange(location: insStart, length: ins.utf16.count), important: !unimportant)) }
            del = ""; ins = ""
        }
        for o in rawOps(a, b) {
            let len = o.text.utf16.count
            switch o.kind {
            case .same:
                flush()
                pa += len
                pb += len
            case .del:
                if del.isEmpty { delStart = pa }
                if ins.isEmpty { insStart = pb }
                del += o.text
                pa += len
            case .ins:
                if ins.isEmpty { insStart = pb }
                if del.isEmpty { delStart = pa }
                ins += o.text
                pb += len
            }
        }
        flush()
        return (left, right)
    }
}
