import Foundation

public enum PopupFuzzy {
    private static let scoreMatch: Int16 = 16
    private static let bonusBoundary: Int16 = scoreMatch / 2
    private static let bonusBoundaryWhite: Int16 = bonusBoundary + 2

    private static func matchToken(_ token: [Character], _ text: [Character])
        -> (score: Int, length: Int, positions: [Int])? {
        let len = token.count
        guard len > 0, len <= text.count else { return nil }
        var bestStart = -1
        var bestScore = Int.min
        var i = 0
        while i + len <= text.count {
            var equal = true
            for k in 0..<len where text[i + k] != token[k] {
                equal = false
                break
            }
            if equal {
                let isWordStart = i == 0 || !(text[i - 1].isLetter || text[i - 1].isNumber)
                let sc = Int(isWordStart ? bonusBoundaryWhite : 0) - i / 8
                if sc > bestScore {
                    bestScore = sc
                    bestStart = i
                }
            }
            i += 1
        }
        guard bestStart >= 0 else { return nil }
        let positions = (0..<len).map { bestStart + $0 }
        return (Int(bestScore) + Int(scoreMatch) * len, len, positions)
    }

    private static func tokens(of query: String) -> [String] {
        query.lowercased()
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)
    }

    public static func score(_ query: String, against text: String) -> Double? {
        let ts = tokens(of: query)
        guard !ts.isEmpty else { return 0 }
        let t = Array(text.lowercased())
        var total = 0
        for tok in ts {
            guard let m = matchToken(Array(tok), t) else { return nil }
            total += m.score
        }
        return Double(total)
    }

    public static func filter<T>(_ rows: [T], query: String,
                                 search: (T) -> String) -> [T] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return rows }
        return FuzzyIndex(rows.map(search)).ranked(q).map { rows[$0] }
    }

    public static func matchRanges(_ query: String, against text: String) -> [NSRange]? {
        let ts = tokens(of: query)
        guard !ts.isEmpty else { return [] }
        let t = Array(text.lowercased())
        var hits: [NSRange] = []
        for tok in ts {
            guard let m = matchToken(Array(tok), t) else { return nil }
            for p in m.positions {
                hits.append(NSRange(location: p, length: 1))
            }
        }
        var merged: [NSRange] = []
        for r in hits.sorted(by: { $0.location < $1.location }) {
            if let last = merged.last, NSMaxRange(last) == r.location {
                merged[merged.count - 1] = NSRange(location: last.location,
                                                   length: last.length + 1)
            } else {
                merged.append(r)
            }
        }
        return merged
    }
}

public final class FuzzyIndex {
    private let keys: [[UInt8]]
    private var lastTokens: [[UInt8]] = []
    private var lastMatches: [Int] = []

    public var count: Int { keys.count }

    public init(_ texts: [String]) {
        keys = texts.map { Array($0.lowercased().utf8) }
    }

    public static func tokens(of query: String) -> [[UInt8]] {
        query.lowercased()
            .split(whereSeparator: { $0.isWhitespace })
            .map { Array($0.utf8) }
    }

    public func ranked(_ query: String) -> [Int] {
        let ts = Self.tokens(of: query)
        guard !ts.isEmpty else {
            lastTokens = []
            lastMatches = []
            return Array(keys.indices)
        }
        let narrowing = !lastTokens.isEmpty && lastTokens.allSatisfy { old in
            ts.contains { Self.contains($0, old) }
        }
        let candidates = narrowing ? lastMatches : Array(keys.indices)
        var scores = [Int](repeating: 0, count: candidates.count)
        var hit = [Bool](repeating: false, count: candidates.count)
        let chunks = candidates.count >= 4000 ? 8 : 1
        let per = (candidates.count + chunks - 1) / max(1, chunks)
        scores.withUnsafeMutableBufferPointer { sp in
            hit.withUnsafeMutableBufferPointer { hp in
                DispatchQueue.concurrentPerform(iterations: chunks) { c in
                    let lo = c * per, hi = min(candidates.count, lo + per)
                    guard lo < hi else { return }
                    for j in lo..<hi {
                        if let s = score(ts, keys[candidates[j]]) {
                            sp[j] = s
                            hp[j] = true
                        }
                    }
                }
            }
        }
        var matches: [(row: Int, score: Int)] = []
        matches.reserveCapacity(candidates.count)
        for j in candidates.indices where hit[j] { matches.append((candidates[j], scores[j])) }
        lastTokens = ts
        lastMatches = matches.map(\.row)
        matches.sort { $0.score != $1.score ? $0.score > $1.score : $0.row < $1.row }
        return matches.map(\.row)
    }

    private func score(_ ts: [[UInt8]], _ text: [UInt8]) -> Int? {
        var total = 0
        for t in ts {
            guard let s = Self.matchToken(t, text) else { return nil }
            total += s
        }
        return total
    }

    static func matchToken(_ tok: [UInt8], _ text: [UInt8]) -> Int? {
        let len = tok.count, n = text.count
        guard len > 0, len <= n else { return nil }
        var best = Int.min
        text.withUnsafeBytes { tb in
            tok.withUnsafeBytes { kb in
                let base = tb.baseAddress!, key = kb.baseAddress!
                let bytes = tb.bindMemory(to: UInt8.self)
                let first = Int32(tok[0])
                var i = 0
                var cAt = 0, cIdx = 0
                while i + len <= n {
                    guard let p = memchr(base + i, first, n - len + 1 - i) else { break }
                    let j = base.distance(to: UnsafeRawPointer(p))
                    while cAt < j {
                        if bytes[cAt] & 0xC0 != 0x80 { cIdx += 1 }
                        cAt += 1
                    }
                    if 10 - cIdx / 8 <= best { break }
                    if memcmp(base + j, key, len) == 0 {
                        let ws = j == 0 || isBoundary(text, before: j)
                        let sc = (ws ? 10 : 0) - cIdx / 8
                        if sc > best { best = sc }
                        if ws { break }
                    }
                    i = j + 1
                }
            }
        }
        return best == Int.min ? nil : best + 16 * len
    }

    private static func isBoundary(_ t: [UInt8], before j: Int) -> Bool {
        let b = t[j - 1]
        if b < 0x80 {
            return !((b >= 0x61 && b <= 0x7A) || (b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5A))
        }
        var k = j - 1
        while k > 0 && t[k] & 0xC0 == 0x80 { k -= 1 }
        guard let c = String(decoding: t[k..<j], as: UTF8.self).first else { return true }
        return !(c.isLetter || c.isNumber)
    }

    static func contains(_ hay: [UInt8], _ needle: [UInt8]) -> Bool {
        guard needle.count <= hay.count else { return false }
        if needle.isEmpty { return true }
        return hay.withUnsafeBytes { h in
            needle.withUnsafeBytes { nd in
                memmem(h.baseAddress!, hay.count, nd.baseAddress!, needle.count) != nil
            }
        }
    }
}

public enum SortRank {
    public static func ranks(_ values: [String], ascending: Bool) -> [Int] {
        var ranks = [Int](repeating: Int.max, count: values.count)
        let filled = values.indices.filter { !values[$0].isEmpty }
            .sorted { values[$0].localizedStandardCompare(values[$1]) == .orderedAscending }
        var r = 0
        for (n, i) in filled.enumerated() {
            if n > 0, values[filled[n - 1]].localizedStandardCompare(values[i]) != .orderedSame { r += 1 }
            ranks[i] = ascending ? r : -r
        }
        return ranks
    }

    public static func order(_ rows: [Int], by ranks: [Int]) -> [Int] {
        rows.enumerated()
            .sorted { a, b in
                let x = ranks[a.element], y = ranks[b.element]
                return x != y ? x < y : a.offset < b.offset
            }
            .map(\.element)
    }
}
