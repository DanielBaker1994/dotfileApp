// sources: DocTemplates.swift
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
        let css = ":root { --p-bg: #fff; }\n:root:has(.paper) {\n  --p-bg: #fff; --p-text: #000;\n}\n:root:has(.doc) body { page: doc; }\n:root:has(.nord) { --p-bg: #2e3440; }\n:root:has(.dracula) { --p-text: #fff; }"
        check(DocTemplates.fromCSS(css) == ["paper", "nord"], "palette blocks only (needs --p-bg, not .doc): \(DocTemplates.fromCSS(css))")
        check(DocTemplates.names(nil, css: css) == ["paper", "nord"], "names come from the css")
        check(DocTemplates.names("x, y", css: css) == ["x", "y"], "config overrides the css")
        check(DocTemplates.names(nil, css: "") == DocTemplates.builtin, "no css palettes = built-in")
        print("doc templates: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
