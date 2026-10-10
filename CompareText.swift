import Foundation

enum CompareSide: String {
    case left, right
    var other: CompareSide { self == .left ? .right : .left }
}

enum TextEncodingKind: String {
    case utf8 = "UTF-8", utf8BOM = "UTF-8 BOM", utf16LE = "UTF-16 LE", utf16BE = "UTF-16 BE", latin1 = "Latin-1"
}

enum EOL: UInt8 {
    case none = 0, lf, crlf, cr
    var label: String {
        switch self {
        case .none: return "none"
        case .lf: return "LF"
        case .crlf: return "CRLF"
        case .cr: return "CR"
        }
    }
    static func from(_ v: Int) -> EOL { EOL(rawValue: UInt8(clamping: v)) ?? .lf }
}

struct Importance: Equatable {
    var leadingWS = true
    var trailingWS = true
    var embeddedWS = false
    var ignoreCase = false
    var lineEndings = true
    var blankLines = false

    init() {}

    init(leadingWS: Bool, trailingWS: Bool, embeddedWS: Bool, ignoreCase: Bool,
         lineEndings: Bool, blankLines: Bool) {
        self.leadingWS = leadingWS
        self.trailingWS = trailingWS
        self.embeddedWS = embeddedWS
        self.ignoreCase = ignoreCase
        self.lineEndings = lineEndings
        self.blankLines = blankLines
    }

    static let exact = Importance(leadingWS: false, trailingWS: false, embeddedWS: false,
                                  ignoreCase: false, lineEndings: false, blankLines: false)

    var json: [String: Any] {
        ["leadingWS": leadingWS, "trailingWS": trailingWS, "embeddedWS": embeddedWS,
         "ignoreCase": ignoreCase, "lineEndings": lineEndings, "blankLines": blankLines]
    }

    init(json: [String: Any]) {
        leadingWS = json["leadingWS"] as? Bool ?? true
        trailingWS = json["trailingWS"] as? Bool ?? true
        embeddedWS = json["embeddedWS"] as? Bool ?? false
        ignoreCase = json["ignoreCase"] as? Bool ?? false
        lineEndings = json["lineEndings"] as? Bool ?? true
        blankLines = json["blankLines"] as? Bool ?? false
    }

    var cacheKey: String {
        "\(leadingWS)\(trailingWS)\(embeddedWS)\(ignoreCase)\(lineEndings)\(blankLines)"
    }
}

struct TextSide {
    var lines: [String] = []
    var eols: [EOL] = []
    var encoding: TextEncodingKind = .utf8

    init() {}

    init(json: [String: Any]) {
        lines = json["lines"] as? [String] ?? []
        eols = (json["eols"] as? [Int] ?? []).map(EOL.from)
        encoding = TextEncodingKind(rawValue: json["encoding"] as? String ?? "utf8") ?? .utf8
    }

    init(text: String) {
        if let side = TextSide.helperSide(Data(text.utf8)) {
            self = side
        }
    }

    var json: [String: Any] {
        ["lines": lines, "eols": eols.map { Int($0.rawValue) }, "encoding": encoding.rawValue]
    }

    var eolLabel: String {
        let kinds = Set(eols.filter { $0 != .none })
        if kinds.count > 1 { return "mixed" }
        if let only = kinds.first { return only.label }
        var counts = [0, 0, 0, 0]
        for e in eols.prefix(5000) { counts[Int(e.rawValue)] += 1 }
        var best = 1
        for i in 1...3 where counts[i] > counts[best] { best = i }
        return (counts[best] == 0 ? EOL.lf : EOL.from(best)).label
    }

    var text: String {
        var out = ""
        out.reserveCapacity(lines.reduce(0) { $0 + $1.utf8.count + 2 })
        for (i, line) in lines.enumerated() {
            out += line
            switch eols.count > i ? eols[i] : EOL.none {
            case .none: break
            case .lf: out += "\n"
            case .crlf: out += "\r\n"
            case .cr: out += "\r"
            }
        }
        return out
    }

    static func helperSide(_ data: Data) -> TextSide? {
        guard case .success(let box) = PythonHelper.shared.callSync(
                "compare.decode", ["data": data.base64EncodedString()], timeout: 60),
              let dict = box as? [String: Any],
              (dict["binary"] as? Bool) == false,
              let side = dict["side"] as? [String: Any] else { return nil }
        return TextSide(json: side)
    }

    static func isBinary(_ data: Data) -> Bool {
        guard case .success(let box) = PythonHelper.shared.callSync(
                "compare.decode", ["data": data.base64EncodedString()], timeout: 60),
              let dict = box as? [String: Any] else { return false }
        return dict["binary"] as? Bool ?? false
    }

    static func decode(_ data: Data) -> TextSide? { helperSide(data) }

    func encoded() -> Data? {
        guard case .success(let box) = PythonHelper.shared.callSync(
                "compare.encode", ["side": json], timeout: 60),
              let dict = box as? [String: Any], let b64 = dict["data"] as? String else { return nil }
        return Data(base64Encoded: b64)
    }
}

enum RowKind: UInt8 { case same = 0, changed, leftOnly, rightOnly }

struct CompareRow: Equatable {
    var l: Int32
    var r: Int32
    var kind: RowKind
    var important: Bool
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

struct TextCompare {
    var left = TextSide()
    var right = TextSide()
    var importance = Importance()
    var ignoreUnimportant = false
    private(set) var rows: [CompareRow] = []
    private(set) var sections: [CompareSection] = []
    private(set) var anchors: [(l: Int, r: Int)] = []
    private(set) var undoCounts: [Int] = [0, 0]
    private(set) var redoCounts: [Int] = [0, 0]
    private var handle = -1

    init() {}

    init(left: TextSide, right: TextSide, importance: Importance = Importance(),
         ignoreUnimportant: Bool = false) {
        self.left = left
        self.right = right
        self.importance = importance
        self.ignoreUnimportant = ignoreUnimportant
        ensureHandle()
    }

    func side(_ s: CompareSide) -> TextSide { s == .left ? left : right }

    private mutating func ensureHandle() {
        guard handle < 0 else { return }
        guard case .success(let box) = PythonHelper.shared.callSync(
                "compare.new",
                ["left": left.json, "right": right.json, "importance": importance.json,
                 "ignoreUnimportant": ignoreUnimportant], timeout: 120),
              let snap = box as? [String: Any] else { return }
        applySnapshot(snap)
    }

    @discardableResult
    private mutating func call(_ method: String, _ params: [String: Any],
                               timeout: TimeInterval = 120) -> Bool {
        ensureHandle()
        guard handle >= 0 else { return false }
        var p = params
        p["handle"] = handle
        guard case .success(let box) = PythonHelper.shared.callSync(method, p, timeout: timeout),
              let snap = box as? [String: Any] else { return false }
        applySnapshot(snap)
        return snap["changed"] as? Bool ?? true
    }

    private mutating func applySnapshot(_ snap: [String: Any]) {
        if let h = snap["handle"] as? Int { handle = h }
        if let j = snap["left"] as? [String: Any] { left = TextSide(json: j) }
        if let j = snap["right"] as? [String: Any] { right = TextSide(json: j) }
        rows = (snap["rows"] as? [[Any]] ?? []).compactMap { r in
            guard r.count >= 3, let l = r[0] as? Int, let rr = r[1] as? Int, let flags = r[2] as? Int
            else { return nil }
            return CompareRow(l: Int32(l), r: Int32(rr),
                              kind: RowKind(rawValue: UInt8(flags & 3)) ?? .same,
                              important: (flags & 4) != 0)
        }
        sections = (snap["sections"] as? [[Any]] ?? []).compactMap { s in
            guard s.count >= 3, let lo = s[0] as? Int, let hi = s[1] as? Int, let imp = s[2] as? Int
            else { return nil }
            return CompareSection(rows: lo..<hi, important: imp != 0)
        }
        anchors = (snap["anchors"] as? [[Any]] ?? []).compactMap { a in
            guard a.count == 2, let l = a[0] as? Int, let r = a[1] as? Int else { return nil }
            return (l: l, r: r)
        }
        undoCounts = snap["undo"] as? [Int] ?? [0, 0]
        redoCounts = snap["redo"] as? [Int] ?? [0, 0]
        if let j = snap["importance"] as? [String: Any] { importance = Importance(json: j) }
        if let on = snap["ignoreUnimportant"] as? Bool { ignoreUnimportant = on }
    }

    mutating func recompute() {
        call("compare.recompute", [:])
    }

    mutating func replace(_ side: CompareSide, _ range: Range<Int>, with new: [String]) {
        call("compare.replace", ["side": side.rawValue, "start": range.lowerBound,
                                 "count": range.count, "lines": new])
    }

    mutating func replace(_ side: CompareSide, _ range: Range<Int>, lines new: [String], eols: [EOL]) {
        call("compare.replace", ["side": side.rawValue, "start": range.lowerBound,
                                 "count": range.count, "lines": new,
                                 "eols": eols.map { Int($0.rawValue) }])
    }

    func undoCount(_ s: CompareSide) -> Int { undoCounts[s == .left ? 0 : 1] }
    func hasUndo(_ s: CompareSide) -> Bool { undoCount(s) > 0 }
    var canUndo: Bool { undoCounts[0] + undoCounts[1] > 0 }
    var canRedo: Bool { redoCounts[0] + redoCounts[1] > 0 }

    @discardableResult
    mutating func undo(_ side: CompareSide? = nil) -> Bool {
        if let side, !hasUndo(side) { return false }
        guard canUndo else { return false }
        call("compare.undo", side.map { ["side": $0.rawValue] } ?? [:])
        return true
    }

    @discardableResult
    mutating func redo() -> Bool {
        guard canRedo else { return false }
        call("compare.redo", [:])
        return true
    }

    @discardableResult
    mutating func copyRows(_ range: Range<Int>, from: CompareSide) -> Bool {
        call("compare.copy_rows", ["lo": range.lowerBound, "hi": range.upperBound,
                                   "from": from.rawValue])
    }

    @discardableResult
    mutating func copySection(_ i: Int, from: CompareSide) -> Bool {
        call("compare.copy_section", ["index": i, "from": from.rawValue])
    }

    mutating func align(left l: Int, right r: Int) {
        call("compare.align", ["l": l, "r": r])
    }

    mutating func clearAlignment(row: Int? = nil) {
        call("compare.clear_alignment", row.map { ["row": $0] } ?? [:])
    }

    mutating func setImportance(_ imp: Importance) {
        importance = imp
        call("compare.set_importance", ["importance": imp.json])
    }

    mutating func setIgnoreUnimportant(_ on: Bool) {
        ignoreUnimportant = on
        call("compare.set_ignore_unimportant", ["on": on])
    }

    mutating func swapSides() {
        call("compare.swap_sides", [:])
    }

    mutating func setSide(_ s: CompareSide, _ t: TextSide) {
        call("compare.set_side", ["side": s.rawValue, "text": t.json])
    }

    @discardableResult
    mutating func trimTrailingWhitespace(_ s: CompareSide) -> Int {
        ensureHandle()
        guard handle >= 0,
              case .success(let box) = PythonHelper.shared.callSync(
                "compare.trim", ["handle": handle, "side": s.rawValue], timeout: 120),
              let snap = box as? [String: Any] else { return 0 }
        let n = snap["count"] as? Int ?? 0
        applySnapshot(snap)
        return n
    }

    @discardableResult
    mutating func convertLineEndings(_ s: CompareSide, to eol: EOL) -> Int {
        ensureHandle()
        guard handle >= 0, eol != .none,
              case .success(let box) = PythonHelper.shared.callSync(
                "compare.convert", ["handle": handle, "side": s.rawValue,
                                    "eol": Int(eol.rawValue)], timeout: 120),
              let snap = box as? [String: Any] else { return 0 }
        let n = snap["count"] as? Int ?? 0
        applySnapshot(snap)
        return n
    }

    func isDiff(_ r: CompareRow) -> Bool {
        r.kind != .same && (r.important || !ignoreUnimportant)
    }

    var importantCount: Int { sections.filter(\.important).count }
    var unimportantCount: Int { sections.count - importantCount }
    var identicalText: Bool { rows.allSatisfy { $0.kind == .same } }

    func section(at row: Int) -> Int? {
        var lo = 0, hi = sections.count - 1
        while lo <= hi {
            let mid = (lo + hi) / 2
            let s = sections[mid].rows
            if row < s.lowerBound { hi = mid - 1 } else if row >= s.upperBound { lo = mid + 1 } else { return mid }
        }
        return nil
    }

    func nextSection(after row: Int) -> Int? { sections.firstIndex { $0.rows.lowerBound > row } }
    func prevSection(before row: Int) -> Int? { sections.lastIndex { $0.rows.lowerBound < row } }

    func lineIndex(_ side: CompareSide, atRow row: Int) -> Int {
        var i = row
        while i < rows.count {
            let v = rows[i].line(side)
            if v >= 0 { return v }
            i += 1
        }
        return side == .left ? left.lines.count : right.lines.count
    }

    func lineRange(_ side: CompareSide, rows range: Range<Int>) -> Range<Int> {
        let start = lineIndex(side, atRow: range.lowerBound)
        let end = range.upperBound >= rows.count
            ? (side == .left ? left.lines.count : right.lines.count)
            : lineIndex(side, atRow: range.upperBound)
        return start..<max(start, end)
    }

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

    func isAnchor(row: Int) -> Bool {
        guard rows.indices.contains(row) else { return false }
        let rr = rows[row]
        return anchors.contains { $0.l == Int(rr.l) && $0.r == Int(rr.r) }
    }
}

enum BinaryCompare {
    /// The byte compare lives in pylib/compare_text.py; cold path (the
    /// binary-files view), so one round trip per pair is fine.
    static func firstDifference(_ a: Data, _ b: Data) -> Int? {
        if case .success(let box) = PythonHelper.shared.callSync(
                "compare.binary_first_difference",
                ["a": a.base64EncodedString(), "b": b.base64EncodedString()], timeout: 30),
           let d = box as? [String: Any] {
            return d["at"] as? Int
        }
        return nil
    }
}

enum CharDiff {
    enum Kind { case same, del, ins }
    struct Op { var kind: Kind; var text: String }

    private static var memo: [String: (left: [Mark], right: [Mark])] = [:]
    private static let memoLock = NSLock()

    static func marks(_ a: String, _ b: String, _ imp: Importance) -> (left: [Mark], right: [Mark]) {
        let key = "\(imp.cacheKey)\u{1}\(a)\u{1}\(b)"
        memoLock.lock()
        if let hit = memo[key] { memoLock.unlock(); return hit }
        memoLock.unlock()
        guard case .success(let box) = PythonHelper.shared.callSync(
                "compare.marks", ["a": a, "b": b, "importance": imp.json], timeout: 30),
              let dict = box as? [String: Any] else { return ([], []) }
        let result = (parseMarks(dict["left"]), parseMarks(dict["right"]))
        memoLock.lock()
        if memo.count > 4096 { memo.removeAll() }
        memo[key] = result
        memoLock.unlock()
        return result
    }

    private static func parseMarks(_ value: Any?) -> [Mark] {
        (value as? [[String: Any]] ?? []).compactMap { m in
            guard let loc = m["location"] as? Int, let len = m["length"] as? Int else { return nil }
            return Mark(range: NSRange(location: loc, length: len),
                        important: m["important"] as? Bool ?? false)
        }
    }

    static func diff(_ a: String, _ b: String) -> [Op] {
        guard case .success(let box) = PythonHelper.shared.callSync(
                "compare.char_diff", ["a": a, "b": b], timeout: 30),
              let dict = box as? [String: Any] else { return [] }
        return (dict["ops"] as? [[Any]] ?? []).compactMap { o in
            guard o.count == 2, let k = o[0] as? Int, let t = o[1] as? String else { return nil }
            return Op(kind: k == 0 ? .same : k == 1 ? .del : .ins, text: t)
        }
    }

    static func changes(_ ops: [Op]) -> Int {
        var n = 0
        var inChange = false
        for o in ops {
            if o.kind == .same { inChange = false } else if !inChange { n += 1; inChange = true }
        }
        return n
    }

    struct Mark: Equatable {
        var range: NSRange
        var important: Bool
    }
}
