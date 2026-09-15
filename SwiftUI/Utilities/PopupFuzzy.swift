import AppKit

// MARK: - Fuzzy search (pure Swift, ported from PopupWindow.swift)

enum PopupFuzzy {
    private static let scoreMatch: Int16 = 16
    private static let bonusBoundary: Int16 = scoreMatch / 2
    private static let bonusBoundaryWhite: Int16 = bonusBoundary + 2

    private static func matchToken(_ token: [Character], _ text: [Character])
        -> (score: Int, length: Int)? {
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
        return (Int(bestScore) + Int(scoreMatch) * len, len)
    }

    private static func tokens(of query: String) -> [String] {
        query.lowercased()
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)
    }

    static func score(_ query: String, against text: String) -> Double? {
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

    static func filter<T>(_ rows: [T], query: String, search: (T) -> String) -> [T] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return rows }
        let ts = tokens(of: q)
        let textCache = rows.map { Array(search($0).lowercased()) }
        let scored: [(T, Int, Int)] = rows.enumerated().compactMap { i, row in
            var total = 0
            var length = 0
            for tok in ts {
                guard let m = matchToken(Array(tok), textCache[i]) else { return nil }
                total += m.score
                length += m.length
            }
            return (row, total, length)
        }
        return scored
            .sorted { a, b in
                if a.1 != b.1 { return a.1 > b.1 }
                if a.2 != b.2 { return a.2 < b.2 }
                return false
            }
            .map { $0.0 }
    }

    static func matchRanges(_ query: String, against text: String) -> [NSRange]? {
        let ts = tokens(of: query)
        guard !ts.isEmpty else { return [] }
        let t = Array(text.lowercased())
        var hits: [NSRange] = []
        for tok in ts {
            guard let m = matchToken(Array(tok), t) else { return nil }
            for p in 0..<tok.count {
                hits.append(NSRange(location: m.length > 0 ? (hits.last.map { NSMaxRange($0) } ?? 0) + p : 0, length: 1))
            }
        }
        // Simplified: return contiguous range of the match
        if hits.isEmpty { return [] }
        let start = hits.first!.location
        let end = NSMaxRange(hits.last!)
        return [NSRange(location: start, length: end - start)]
    }
}
