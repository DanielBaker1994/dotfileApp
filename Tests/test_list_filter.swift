// sources: ListFilter.swift
// The list / table filter box (ListFilter.swift): FuzzyIndex gives the same
// rows in the same order as the old per-keystroke [Character] matcher (kept
// below as OldFuzzy, the reference), incremental narrowing never drops a
// match, SortRank orders like the old localizedStandardCompare sort — and a
// keystroke over 20k Jira-sized rows stays inside one frame.
// Usage: bin/run-tests.sh filter
//   WS_FILTER_JSON=FILE  use a tab file (bin/fake-jira-tab.sh writes one)
//   WS_FILTER_ROWS=N     synthetic row count (default 20000)

import Foundation

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_list_filter.swift:\(line))")
    }
}

func ms(_ f: () -> Void) -> Double {
    let t = DispatchTime.now().uptimeNanoseconds
    f()
    return Double(DispatchTime.now().uptimeNanoseconds - t) / 1e6
}

// the app's searchText: the [jira] filter fields + filterable columns, each
// capped at 150 characters, joined with spaces (loadListItems)
let searchFields = ["key", "title", "status", "assignee", "reporter", "description", "labels",
                    "priority", "releaseLabel", "project"]
func searchText(_ d: [String: String]) -> String {
    searchFields.compactMap { d[$0].map { String($0.prefix(150)) } }.joined(separator: " ")
}

// deterministic rows shaped like jira/fake_jira_tab.py's
struct LCG {
    var s: UInt64
    mutating func next(_ n: Int) -> Int {
        s = s &* 6364136223846793005 &+ 1442695040888963407
        return Int((s >> 33) % UInt64(n))
    }
}
func syntheticRows(_ n: Int) -> [[String: String]] {
    let words = ("lorem ipsum dolor sit amet payment processor invoice retry checkout service "
        + "gateway timeout login session token refresh cache layer report export import "
        + "dashboard widget mobile crash memory leak search index query slow api migrate "
        + "schema billing customer webhook queue worker deploy pipeline flaky test café "
        + "résumé naïve über straße").split(separator: " ").map(String.init)
    let people = ["Ana Lopez", "Bo Chen", "Cara Diaz", "Dev Patel", "Eli Novak", "Fay Ober", ""]
    let statuses = ["To Do", "In Progress", "In Review", "Done", "Blocked"]
    var r = LCG(s: 42)
    func sentence(_ k: Int) -> String { (0..<k).map { _ in words[r.next(words.count)] }.joined(separator: " ") }
    return (0..<n).map { i in
        let p = ["PAY", "WEB", "OPS", "MOB"][r.next(4)]
        return ["key": "\(p)-\(i + 1)", "title": sentence(4 + r.next(6)).capitalized,
                "status": statuses[r.next(statuses.count)], "assignee": people[r.next(people.count)],
                "reporter": people[r.next(people.count - 1)], "description": sentence(r.next(200)),
                "labels": r.next(3) == 0 ? "" : words[r.next(words.count)], "priority": "P\(r.next(5))",
                "releaseLabel": r.next(2) == 0 ? "" : "\(r.next(20)).\(r.next(9))", "project": p,
                "updated": "2026-0\(1 + r.next(9))-1\(r.next(10))T10:0\(r.next(10)):00.000-0400"]
    }
}

func loadRows() -> [[String: String]] {
    if let f = ProcessInfo.processInfo.environment["WS_FILTER_JSON"],
       let data = FileManager.default.contents(atPath: (f as NSString).expandingTildeInPath),
       let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] {
        print("rows from \(f)")
        return arr.map { $0.compactMapValues { $0 as? String } }
    }
    return syntheticRows(Int(ProcessInfo.processInfo.environment["WS_FILTER_ROWS"] ?? "") ?? 20000)
}

// the old filteredRows sort (stable, blanks last)
func oldSort(_ rows: [Int], _ vals: [String], ascending: Bool) -> [Int] {
    rows.enumerated().sorted { a, b in
        let x = vals[a.element], y = vals[b.element]
        if x.isEmpty != y.isEmpty { return y.isEmpty }
        let c = x.localizedStandardCompare(y)
        if c == .orderedSame { return a.offset < b.offset }
        return ascending ? c == .orderedAscending : c == .orderedDescending
    }.map { $0.element }
}

func testMatching() {
    let texts = ["Payment retry", "the payment", "xpayment", "café crème", "naïve-bayes", "zz pay ment",
                 "PAY-12 Checkout", "über-fast", "ÜBER"]
    let idx = FuzzyIndex(texts)
    for q in ["pay", "payment", "pay ment", "café", "bayes", "über", "PAY-1", "ment pay", "nothing", "é", ""] {
        let old = OldFuzzy.filter(Array(texts.indices), query: q) { texts[$0] }
        check(idx.ranked(q) == old, "'\(q)': \(idx.ranked(q)) vs old \(old)")
    }
    check(FuzzyIndex(["ab"]).ranked("abc").isEmpty, "a token longer than the text")
    check(PopupFuzzy.filter(["b", "a b", "c"], query: "b") { $0 } == ["b", "a b"], "PopupFuzzy.filter delegates")
}

func testSortRank() {
    let vals = ["b", "", "a10", "a2", "B", "a2", "", "c"]
    for asc in [true, false] {
        let ranks = SortRank.ranks(vals, ascending: asc)
        for rows in [Array(vals.indices), [7, 1, 3, 5, 0, 2], [6, 5, 4, 3, 2, 1, 0]] {
            check(SortRank.order(rows, by: ranks) == oldSort(rows, vals, ascending: asc),
                  "sort \(asc ? "asc" : "desc") \(rows)")
        }
    }
}

func testBigList() {
    let rows = loadRows()
    let texts = rows.map(searchText)
    let n = texts.count
    var index: FuzzyIndex!
    let build = ms { index = FuzzyIndex(texts) }
    print(String(format: "%d rows, index built in %.1f ms", n, build))

    // typed letter by letter, then a few backspaces and a second word
    let typed = "payment"
    var queries = (1...typed.count).map { String(typed.prefix($0)) }
    queries += ["paymen", "payme", "paym", "paym r", "paym re", "paym ret", "p", "", "e", "es", "est"]
    var oldMax = 0.0, newMax = 0.0, oldTotal = 0.0, newTotal = 0.0
    let ids = Array(0..<n)
    for q in queries {
        var old: [Int] = [], new: [Int] = []
        let to = ms { old = OldFuzzy.filter(ids, query: q) { texts[$0] } }
        let tn = ms { new = index.ranked(q) }
        check(new == old, "'\(q)': \(new.count) rows vs old \(old.count) (same order)")
        oldMax = max(oldMax, to); newMax = max(newMax, tn)
        oldTotal += to; newTotal += tn
        print(String(format: "  %-10@ %6d rows  old %7.1f ms  new %6.2f ms", "'\(q)'" as NSString, new.count, to, tn))
    }
    print(String(format: "per keystroke: old max %.1f ms (total %.0f), new max %.2f ms (total %.1f)",
                 oldMax, oldTotal, newMax, newTotal))
    check(newMax < 16, String(format: "a keystroke over %d rows fits a frame (%.1f ms)", n, newMax))

    // header sort: one rank pass, then integer sorts per keystroke
    let vals = rows.map { $0["updated"] ?? "" }
    var ranks: [Int] = []
    let rankBuild = ms { ranks = SortRank.ranks(vals, ascending: false) }
    let matched = index.ranked("e")
    var old: [Int] = [], new: [Int] = []
    let to = ms { old = oldSort(matched, vals, ascending: false) }
    let tn = ms { new = SortRank.order(matched, by: ranks) }
    check(new == old, "sorted by updated: same order")
    print(String(format: "sort %d matches: old %.1f ms, new %.2f ms (ranks built once in %.1f ms)",
                 matched.count, to, tn, rankBuild))
    check(tn < 16, "sorting a keystroke's matches fits a frame")
}

@main
struct ListFilterTests {
    static func main() {
        testMatching()
        testSortRank()
        testBigList()
        print("\n\(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}

// MARK: - the matcher before ListFilter.swift (reference + baseline timing)

enum OldFuzzy {
    // --- scoring constants (from fzf's algo.go, default scheme) ---
    private static let scoreMatch: Int16 = 16
    private static let bonusBoundary: Int16 = scoreMatch / 2
    private static let bonusBoundaryWhite: Int16 = bonusBoundary + 2

    // One token's match against the text (all tokens must match; sum of
    // scores = overall score, total matched length for tiebreaking).
    // LESS PERMISSIVE: a token must appear as a CONTIGUOUS substring
    // (case-insensitive) — a typed word like "magazine" only matches rows
    // that actually contain "magazine", never letters scattered mid-word.
    // Matches at word boundaries are preferred, then earlier matches.
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

    // Total score of the query against the text; nil = not a match.
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

    // Filter rows by fzf score against their searchable text, best first
    // (score desc, then shorter total match, then input order — fzf's
    // default tiebreaks). Empty query returns everything unchanged.
    static func filter<T>(_ rows: [T], query: String,
                                 search: (T) -> String) -> [T] {
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

    // Character ranges of the query's matched characters within `text`
    // (adjacent matches merged). nil = no match.
    static func matchRanges(_ query: String, against text: String) -> [NSRange]? {
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
        // merge adjacent single-character matches into runs
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
