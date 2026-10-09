import Foundation

// The vim "/" search over a pane (VimKeys.swift): which row / text range is
// the next match. Foundation only — tested by `bin/run-tests.sh vim-keys`
// (Tests/test_vim_search.swift).
enum VimSearch {
    // the row holding `query` (ignore case) after `from` (before it when
    // back), wrapping; skipCurrent = never `from` itself unless it is the only
    // one. index = the hit's place among every matching row (1-based)
    static func rows(_ texts: [String], query: String, from: Int, back: Bool, skipCurrent: Bool)
        -> (row: Int?, index: Int, count: Int) {
        let n = texts.count
        guard n > 0, !query.isEmpty else { return (nil, 0, 0) }
        let hits = texts.indices.filter { matches(texts[$0], query) }
        guard !hits.isEmpty else { return (nil, 0, 0) }
        let start = max(0, min(n - 1, from))
        let row: Int
        if back {
            row = hits.last { skipCurrent ? $0 < start : $0 <= start } ?? hits.last!
        } else {
            row = hits.first { skipCurrent ? $0 > start : $0 >= start } ?? hits.first!
        }
        return (row, (hits.firstIndex(of: row) ?? 0) + 1, hits.count)
    }

    // does a row hold `query` (the search's rule: ignore case and accents)
    static func matches(_ text: String, _ query: String) -> Bool {
        !query.isEmpty && text.range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) != nil
    }

    // every match in a text, in order (UTF-16 ranges; at most `limit`) — the
    // search steps through them, the highlights paint them
    static func ranges(_ s: String, query: String, limit: Int = 5000) -> [NSRange] {
        let ns = s as NSString
        guard ns.length > 0, !query.isEmpty else { return [] }
        var all: [NSRange] = []
        var at = 0
        while at < ns.length, all.count < limit {
            let r = ns.range(of: query, options: [.caseInsensitive, .diacriticInsensitive],
                             range: NSRange(location: at, length: ns.length - at))
            if r.location == NSNotFound { break }
            all.append(r)
            at = r.location + max(1, r.length)
        }
        return all
    }

    // the same over a text's characters (UTF-16 offsets, NSRange)
    static func text(_ s: String, query: String, from: Int, back: Bool, skipCurrent: Bool)
        -> (range: NSRange?, index: Int, count: Int) {
        let ns = s as NSString
        let all = ranges(s, query: query)
        guard !all.isEmpty else { return (nil, 0, 0) }
        let start = max(0, min(ns.length, from))
        let hit: NSRange
        if back {
            hit = all.last { skipCurrent ? $0.location < start : $0.location <= start } ?? all.last!
        } else {
            hit = all.first { skipCurrent ? $0.location > start : $0.location >= start } ?? all.first!
        }
        return (hit, (all.firstIndex { $0.location == hit.location } ?? 0) + 1, all.count)
    }

    // Ctrl+W / Opt+Delete in the "/" bar: the last word and the spaces before it
    static func dropWord(_ s: String) -> String {
        var t = s
        while t.last == " " { t.removeLast() }
        while let c = t.last, c != " " { t.removeLast() }
        return t
    }
}
