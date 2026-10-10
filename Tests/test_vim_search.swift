// sources: VimSearch.swift
import Foundation

@main
struct VimSearchTests {
    static var passed = 0
    static var failed = 0

    static func check(_ condition: Bool, _ message: String, line: Int = #line) {
        if condition {
            passed += 1
        } else {
            failed += 1
            print("  FAIL: \(message) (test_vim_search.swift:\(line))")
        }
    }

    static func main() {
        let rows = ["KAN-3 Subtask", "KAN-2 Task 2", "SAM1-4 Billing", "kan-1 task 1", "Résumé draft"]

        var r = VimSearch.rows(rows, query: "kan", from: 0, back: false, skipCurrent: false)
        check(r.row == 0 && r.index == 1 && r.count == 3, "typing 'kan' on row 0 stays on row 0 (1/3), got \(r)")
        r = VimSearch.rows(rows, query: "billing", from: 0, back: false, skipCurrent: false)
        check(r.row == 2 && r.count == 1, "typing finds a later row")
        r = VimSearch.rows(rows, query: "KAN-1", from: 0, back: false, skipCurrent: false)
        check(r.row == 3, "upper-case query matches lower-case row (no smartcase)")
        r = VimSearch.rows(rows, query: "resume", from: 0, back: false, skipCurrent: false)
        check(r.row == 4, "diacritics ignored")

        r = VimSearch.rows(rows, query: "kan", from: 0, back: false, skipCurrent: true)
        check(r.row == 1 && r.index == 2, "n from row 0 → row 1 (2/3)")
        r = VimSearch.rows(rows, query: "kan", from: 3, back: false, skipCurrent: true)
        check(r.row == 0 && r.index == 1, "n from the last match wraps to the first")
        r = VimSearch.rows(rows, query: "kan", from: 3, back: true, skipCurrent: true)
        check(r.row == 1, "N from row 3 → row 1")
        r = VimSearch.rows(rows, query: "kan", from: 0, back: true, skipCurrent: true)
        check(r.row == 3, "N from the first match wraps to the last")
        r = VimSearch.rows(rows, query: "billing", from: 2, back: false, skipCurrent: true)
        check(r.row == 2 && r.count == 1, "n with one match stays on it")
        check(VimSearch.rows(rows, query: "zzz", from: 0, back: false, skipCurrent: false).row == nil, "no match → nil")
        check(VimSearch.rows(rows, query: "", from: 0, back: false, skipCurrent: false).row == nil, "empty query → nil")
        check(VimSearch.rows([], query: "a", from: 0, back: false, skipCurrent: false).row == nil, "no rows → nil")
        check(VimSearch.rows(rows, query: "kan", from: 99, back: false, skipCurrent: true).row == 0,
              "a cursor past the end is clamped (wraps to the first)")

        let text = "alpha beta\nGamma beta\n🙂 beta end"
        var t = VimSearch.text(text, query: "BETA", from: 0, back: false, skipCurrent: false)
        check(t.range == NSRange(location: 6, length: 4) && t.count == 3 && t.index == 1, "text: first 'beta', got \(t)")
        t = VimSearch.text(text, query: "beta", from: 6, back: false, skipCurrent: true)
        check(t.range?.location == 17 && t.index == 2, "text n → the second")
        t = VimSearch.text(text, query: "beta", from: 17, back: false, skipCurrent: true)
        let third = (text as NSString).range(of: "beta end").location
        check(t.range?.location == third && t.index == 3, "text n past an emoji (UTF-16) → the third")
        t = VimSearch.text(text, query: "beta", from: third, back: false, skipCurrent: true)
        check(t.range?.location == 6, "text n wraps")
        t = VimSearch.text(text, query: "beta", from: 6, back: true, skipCurrent: true)
        check(t.range?.location == third, "text N wraps backward")
        check(VimSearch.text(text, query: "nope", from: 0, back: false, skipCurrent: false).range == nil, "text: no match")

        check(VimSearch.dropWord("sprint board ") == "sprint ", "Ctrl+W drops the last word + trailing spaces")
        check(VimSearch.dropWord("sprint") == "", "Ctrl+W on one word clears")
        check(VimSearch.dropWord("") == "", "Ctrl+W on empty")

        check(VimSearch.matches("Résumé draft", "resume"), "row match ignores case + accents")
        check(!VimSearch.matches("KAN-2", ""), "an empty query matches nothing")
        let all = VimSearch.ranges("Kan kan KAN", query: "kan")
        check(all.map { $0.location } == [0, 4, 8], "every text match in order, got \(all)")
        check(VimSearch.ranges(String(repeating: "a", count: 50), query: "a", limit: 10).count == 10, "ranges stop at the limit")

        print("vim search: \(passed) passed, \(failed) failed")
        if failed > 0 { exit(1) }
    }
}
