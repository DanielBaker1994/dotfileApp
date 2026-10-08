// sources: DocTemplates.swift
// Document template marker: parse / switch / remove on the note's first line.
// Usage: bin/run-tests.sh doctemplates

import Foundation

var passed = 0
var failed = 0
func check(_ c: Bool, _ m: String, line: Int = #line) {
    if c { passed += 1 } else { failed += 1; print("  FAIL: \(m) (test_doc_templates.swift:\(line))") }
}

@main
struct Main {
    static func main() {
        let plain = "# Title\n\nbody"
        check(DocTemplates.current(in: plain) == nil, "no marker")
        let a = DocTemplates.apply(plain, template: "paper")
        check(a == "<div class=\"doc paper\"></div>\n\n# Title\n\nbody", "insert: \(a)")
        check(DocTemplates.current(in: a) == "paper", "current after insert")
        let b = DocTemplates.apply(a, template: "terminal")
        check(b == "<div class=\"doc terminal\"></div>\n\n# Title\n\nbody", "switch: \(b)")
        let foot = "<div class=\"doc paper\" data-foot=\"Confidential — A&B\"></div>\n\n# T"
        let c = DocTemplates.apply(foot, template: "executive")
        check(c == "<div class=\"doc executive\" data-foot=\"Confidential — A&B\"></div>\n\n# T", "footer kept: \(c)")
        check(DocTemplates.apply(a, template: nil) == plain, "remove restores the note")
        check(DocTemplates.apply(plain, template: nil) == plain, "remove without marker is a no-op")
        check(DocTemplates.parse("<div class=\"note\"></div>") == nil, "other divs are not markers")
        check(DocTemplates.apply("", template: "paper") == "<div class=\"doc paper\"></div>\n\n", "empty note")
        check(DocTemplates.names(nil) == DocTemplates.builtin, "default names")
        check(DocTemplates.names(" a , b,, ") == ["a", "b"], "configured names")
        print("doc templates: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
