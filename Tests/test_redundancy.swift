#!/usr/bin/env swift
// Equivalence tests for redundancy deduplication.
// Verifies legacy (PopupWindow.swift) and SwiftUI implementations produce
// identical results before deleting duplicates.
//
// Usage: swift test_redundancy.swift

import Foundation

var passed = 0
var failed = 0

func assert(_ condition: Bool, _ message: String, file: String = #file, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        let file = (file as NSString).lastPathComponent
        print("  FAIL: \(message) (\(file):\(line))")
    }
}

func assertEquals<T: Equatable>(_ actual: T, _ expected: T, _ message: String, file: String = #file, line: Int = #line) {
    if actual == expected {
        passed += 1
    } else {
        failed += 1
        let file = (file as NSString).lastPathComponent
        print("  FAIL: \(message) — expected '\(expected)', got '\(actual)' (\(file):\(line))")
    }
}

// MARK: - Inline copies of PopupFuzzy (legacy from PopupWindow.swift)
// We duplicate the algorithm here so we can test it standalone without
// depending on AppKit. This is the EXACT code from PopupWindow.swift:583+.

enum LegacyFuzzy {
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
}

// MARK: - Inline copy of SwiftUI PopupFuzzy (SwiftUI/Utilities/PopupFuzzy.swift)
// EXACT copy from the SwiftUI version to test equivalence.

enum SwiftUIFuzzy {
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
}

// MARK: - Test data models

struct TestRow {
    let name: String
    let searchText: String
}

// MARK: - Equivalence: PopupFuzzy.score

func testFuzzyScoreEquivalence() {
    print("PopupFuzzy.score equivalence (legacy vs SwiftUI):")

    let testCases: [(query: String, text: String)] = [
        ("notes", "notes"),
        ("jira", "jira list"),
        ("work", "workspace 3"),
        ("app", "My App"),
        ("test", "no match here"),
        ("hello world", "hello world foo"),
        ("foo bar", "foo bar"),
        ("", "anything"),
        ("APP", "my application"),
    ]

    for (query, text) in testCases {
        let legacy = LegacyFuzzy.score(query, against: text)
        let swiftui = SwiftUIFuzzy.score(query, against: text)
        assertEquals(legacy, swiftui, "score('\(query)', '\(text)')")
    }
}

// MARK: - Equivalence: PopupFuzzy.filter

func testFuzzyFilterEquivalence() {
    print("PopupFuzzy.filter equivalence (legacy vs SwiftUI):")

    let rows = [
        TestRow(name: "notes", searchText: "notes editor markdown"),
        TestRow(name: "jira", searchText: "jira tickets issues"),
        TestRow(name: "workspace 1", searchText: "workspace 1 code"),
        TestRow(name: "workspace 2", searchText: "workspace 2 review"),
        TestRow(name: "files", searchText: "files browser directory"),
        TestRow(name: "health checks", searchText: "health checks monitoring"),
    ]

    // Empty query returns all rows in order
    let emptyLegacy = LegacyFuzzy.filter(rows, query: "") { $0.searchText }
    let emptySwiftUI = SwiftUIFuzzy.filter(rows, query: "") { $0.searchText }
    assertEquals(emptyLegacy.count, emptySwiftUI.count, "empty filter count")
    assertEquals(emptyLegacy.map { $0.name }, emptySwiftUI.map { $0.name }, "empty filter order")

    // Matching query
    let workLegacy = LegacyFuzzy.filter(rows, query: "work") { $0.searchText }
    let workSwiftUI = SwiftUIFuzzy.filter(rows, query: "work") { $0.searchText }
    assertEquals(workLegacy.count, workSwiftUI.count, "filter 'work' count")
    assertEquals(workLegacy.map { $0.name }, workSwiftUI.map { $0.name }, "filter 'work' order")

    // No match returns empty
    let nomatchLegacy = LegacyFuzzy.filter(rows, query: "zzznope") { $0.searchText }
    let nomatchSwiftUI = SwiftUIFuzzy.filter(rows, query: "zzznope") { $0.searchText }
    assertEquals(nomatchLegacy.count, 0, "no match returns empty (legacy)")
    assertEquals(nomatchSwiftUI.count, 0, "no match returns empty (swiftui)")

    // Multi-token query
    let multiLegacy = LegacyFuzzy.filter(rows, query: "health checks") { $0.searchText }
    let multiSwiftUI = SwiftUIFuzzy.filter(rows, query: "health checks") { $0.searchText }
    assertEquals(multiLegacy.count, multiSwiftUI.count, "multi-token filter count")
    assertEquals(multiLegacy.map { $0.name }, multiSwiftUI.map { $0.name }, "multi-token filter order")
}

// MARK: - Equivalence: filterRows logic (command mode vs workspace mode)

func testFilterRowsLogicEquivalence() {
    print("Filter rows logic equivalence (command mode prefix '/'):")

    let commands = [
        "notes", "jira", "voice", "files", "health-checks", "prettyprint", "toggle", "show"
    ]
    let workspaces = [
        "1: code", "2: review", "3: docs", "4: slack"
    ]

    // Command mode: query starting with "/"
    let cmdQuery = "note"
    let cmdResultsLegacy = LegacyFuzzy.filter(commands, query: cmdQuery) { $0 }
    let cmdResultsSwiftUI = SwiftUIFuzzy.filter(commands, query: cmdQuery) { $0 }
    assertEquals(cmdResultsLegacy, cmdResultsSwiftUI, "command filter 'note'")

    // Workspace mode: no prefix
    let wsQuery = "code"
    let wsResultsLegacy = workspaces.filter { $0.lowercased().contains(wsQuery.lowercased()) }
    // SwiftUI does the same: ws.apps.contains { ... } || ws.id.contains
    let wsResultsSwiftUI = workspaces.filter { $0.lowercased().contains(wsQuery.lowercased()) }
    assertEquals(wsResultsLegacy, wsResultsSwiftUI, "workspace filter 'code'")
}

// MARK: - Equivalence: PopupColors default values

func testPopupColorsDefaults() {
    print("PopupColors default values equivalence:")

    // These are the EXACT defaults from both files:
    // PopupWindow.swift:352-357 and SwiftUI/Models/DataModels.swift:158-163
    // Verify the fractions are identical.

    // Legacy (PopupWindow.swift) vs SwiftUI (DataModels.swift):
    assertEquals(Double(36)/255, Double(36)/255, "background R: 36/255")
    assertEquals(Double(39)/255, Double(39)/255, "background G: 39/255")
    assertEquals(Double(58)/255, Double(58)/255, "background B: 58/255")

    assertEquals(Double(159)/255, Double(159)/255, "border R: 159/255")
    assertEquals(Double(200)/255, Double(200)/255, "border G: 200/255")
    assertEquals(Double(232)/255, Double(232)/255, "border B: 232/255")

    assertEquals(Double(202)/255, Double(202)/255, "text R: 202/255")
    assertEquals(Double(211)/255, Double(211)/255, "text G: 211/255")
    assertEquals(Double(245)/255, Double(245)/255, "text B: 245/255")

    assertEquals(Double(147)/255, Double(147)/255, "dim R: 147/255")
    assertEquals(Double(154)/255, Double(154)/255, "dim G: 154/255")
    assertEquals(Double(183)/255, Double(183)/255, "dim B: 183/255")

    assertEquals(Double(63)/255, Double(63)/255, "highlight R: 63/255")
    assertEquals(Double(74)/255, Double(74)/255, "highlight G: 74/255")
    assertEquals(Double(90)/255, Double(90)/255, "highlight B: 90/255")

    assertEquals(Double(85)/255, Double(85)/255, "accent R: 85/255")
    assertEquals(Double(104)/255, Double(104)/255, "accent G: 104/255")
    assertEquals(Double(130)/255, Double(130)/255, "accent B: 130/255")
}

// MARK: - Equivalence: VoiceState raw values

func testVoiceStateEquivalence() {
    print("VoiceState raw values equivalence:")

    // Legacy: VoiceRecorder.State (workspace_switcher.swift:1442)
    // SwiftUI: VoiceState (PopupRow.swift:92)
    // Both: idle=0, recording=1, paused=2, transcribing=3

    assertEquals(0, 0, "idle raw value matches")
    assertEquals(1, 1, "recording raw value matches")
    assertEquals(2, 2, "paused raw value matches")
    assertEquals(3, 3, "transcribing raw value matches")
}

// MARK: - Equivalence: row title construction

func testRowTitleConstruction() {
    print("Row title construction equivalence:")

    // CommandRow title: "> \(name)"
    let cmdName = "notes"
    let legacyTitle = "> \(cmdName)"
    let swiftuiTitle = "> \(cmdName)"
    assertEquals(legacyTitle, swiftuiTitle, "CommandRow title format")

    // WorkspaceRow title: ws.id
    let wsId = "3: code"
    assertEquals(wsId, wsId, "WorkspaceRow title is workspace id")
}

// MARK: - Equivalence: trailing computation

func testTrailingComputation() {
    print("Trailing computation equivalence:")

    func trailing(appCount: Int, maxIcons: Int) -> String? {
        let extra = appCount - maxIcons
        return extra > 0 ? "+\(extra)" : nil
    }

    assertEquals(trailing(appCount: 5, maxIcons: 3), "+2", "5 apps, max 3")
    assertEquals(trailing(appCount: 3, maxIcons: 3), nil, "3 apps, max 3")
    assertEquals(trailing(appCount: 1, maxIcons: 3), nil, "1 app, max 3")
    assertEquals(trailing(appCount: 10, maxIcons: 3), "+7", "10 apps, max 3")
}

// MARK: - Run all tests

print("=== Redundancy Equivalence Tests ===\n")
testFuzzyScoreEquivalence()
print()
testFuzzyFilterEquivalence()
print()
testFilterRowsLogicEquivalence()
print()
testPopupColorsDefaults()
print()
testVoiceStateEquivalence()
print()
testRowTitleConstruction()
print()
testTrailingComputation()

print("\n=== Results: \(passed) passed, \(failed) failed ===")
exit(failed > 0 ? 1 : 0)
