// sources: AIFormat.swift ProcessRun.swift
// The AI view's text helpers (AIFormat.swift): rule files and their chain,
// comma rows -> tables, the keep-words check. No model needed.
// Usage: bin/run-tests.sh ai      (the model itself: bin/run-tests.sh ai-live)

import Foundation

// the two [ai] settings AIFormat.swift reads (AIWindow.swift in the app)
func aiSetting(_ key: String, _ fallback: String) -> String { fallback }
func aiNumber(_ key: String, _ fallback: CGFloat) -> CGFloat { fallback }

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_ai_format.swift:\(line))")
    }
}

@main
struct Main {
    static func main() {
        // --- comma rows -> tables
        let t = CSVTables.convert("example,example1,example2\n---\n1,2,3")
        check(t == "| example | example1 | example2 |\n| --- | --- | --- |\n| 1 | 2 | 3 |", "header, ---, row: \(t)")
        check(CSVTables.convert("name, status\nweb01, up\nweb02, down").hasPrefix("| name | status |\n| --- | --- |\n| web01 | up |"),
              "three two-column rows need no ---")
        check(CSVTables.convert("a,b,c\n1,2,3").contains("| 1 | 2 | 3 |"), "two three-column rows need no ---")
        let prose = "Hi Raj,\n\nme and John was there, we seen it and it look good.\n\nThanks,\nDan"
        check(CSVTables.convert(prose) == prose, "an email with commas stays an email")
        let two = "Hi team, thanks\nSee you, Dan"
        check(CSVTables.convert(two) == two, "two short comma lines are not a table")
        let fenced = "```\na,b,c\n---\n1,2,3\n```"
        check(CSVTables.convert(fenced) == fenced, "code blocks are left alone")
        let mixed = CSVTables.convert("Status below\nhost,state\n---\nweb01,up\nMore soon.")
        check(mixed == "Status below\n\n| host | state |\n| --- | --- |\n| web01 | up |\n\nMore soon.", "blank lines around: \(mixed)")
        check(CSVTables.convert("a,b\n---\n1,2,3") == "a,b\n---\n1,2,3", "ragged rows are not a table")

        // --- keep-words
        let steps = "We need to do three things: first update the changelog, second run the tests."
        check(WordGuard.check(steps, "We need to do three things:\n\n1. update the changelog\n2. run the tests").ok,
              "a list may drop first/second")
        check(!WordGuard.check("What time is the meeting tomorrow?", "The meeting tomorrow is at 10:00 AM.").ok,
              "an answer to the draft adds words")
        check(!WordGuard.check("Hi team, see you there. You're the best.", "Hi team, see you there.").ok,
              "a dropped sentence is caught")
        check(WordGuard.check("Anna is in Paris, Ben is in Rome.",
                              "| Name | City |\n| --- | --- |\n| Anna | Paris |\n| Ben | Rome |").ok,
              "a table's header row may be new")
        check(WordGuard.check("run [[CODE1]] then **stop**", "- run [[CODE1]]\n- then **stop**").ok, "markup is not words")

        // --- rule files + the chain
        let dir = NSTemporaryDirectory() + "ai-rules-test-\(getpid())"
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: dir) }
        func write(_ name: String, _ text: String) { try? text.write(toFile: dir + "/" + name, atomically: true, encoding: .utf8) }
        write("a.md", "---\nname: A\noutput: diff\nprompt: Proofread this draft:\nthen: b\ncsv-tables: true\n---\nFix the\ngrammar.\n- one")
        write("b.md", "---\nname: B\nkeep-words: true\nthen: a.md\n---\nFormat it.")
        let a = AIRule.load(dir + "/a.md")
        check(a.warnings.isEmpty, "new keys are known: \(a.warnings)")
        check(a.then == "b.md" && a.csvTables && !a.keepWords, "then / csv-tables parsed")
        check(a.wrap("hello") == "Proofread this draft:\n\nhello", "prompt goes before the text")
        check(a.prepare("x,y\n---\n1,2").hasPrefix("| x | y |"), "csv-tables runs in prepare")
        check(a.instructions(guarded: false) == "Fix the grammar.\n- one", "instructions reflowed")
        check(a.preview.hasSuffix("→ then b.md"), "the chain shows in the preview")
        let chain = AIRule.chain(a)
        check(chain.map(\.name) == ["A", "B"], "a -> b, and b -> a stops: \(chain.map(\.name))")
        check(chain[0].warnings.contains { $0.contains("loops") }, "a loop is a warning")
        let b = chain[1]
        check(b.accept(input: "one two", answer: "- one\n- two").note == nil, "keep-words: same words pass")
        let bad = b.accept(input: "one two", answer: "three")
        check(bad.text == "one two" && bad.note != nil, "keep-words: changed words -> the input back, with a note")
        write("c.md", "---\nthen: nope.md\n---\nx")
        check(AIRule.chain(AIRule.load(dir + "/c.md"))[0].warnings.contains { $0.contains("nope.md") }, "a missing then is a warning")

        // --- the shipped rules
        let rules = (#filePath as NSString).deletingLastPathComponent + "/../rules"
        let g = AIRule.chain(AIRule.load(rules + "/grammar-check.md"))
        check(g.map(\.file) == ["grammar-check.md", "markdown-format.md"], "Grammar Check runs Markdown Format after")
        check(g.allSatisfy { $0.warnings.isEmpty }, "no unknown keys: \(g.flatMap(\.warnings))")
        check(g[1].keepWords && !g[0].keepWords, "only the layout step is keep-words")
        check(!g[0].instructions.lowercased().contains("table with") && !g[0].instructions.contains("bullet list"),
              "the grammar rule says nothing about formatting")
        let ask = AIRule.load(rules + "/ask.md")
        check(ask.warnings.isEmpty && !ask.diff && ask.prompt.isEmpty, "Ask: plain, free-form")

        // --- GitHub alerts: pandoc gfm's divs get inline styles (classes don't survive a paste)
        if RichText.available, let h = RichText.html("> [!WARNING]\n> Mind the gap\n", for: .outlook) {
            check(!h.contains("class=\"warning\"") && h.contains("border-left:3px solid #9a6700"),
                  "warning alert styled inline: \(h)")
            check(h.contains("font-weight:bold;color:#9a6700\">Warning") && h.contains("Mind the gap"),
                  "alert title + body kept: \(h)")
        }

        print("test_ai_format: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
