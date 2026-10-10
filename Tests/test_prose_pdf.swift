// sources: PythonHelper.swift AIFormat.swift ProcessRun.swift
import Foundation

func aiSetting(_ key: String, _ fallback: String) -> String { fallback }
func aiNumber(_ key: String, _ fallback: CGFloat) -> CGFloat { fallback }

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition { passed += 1 } else { failed += 1; print("  FAIL: \(message) (test_prose_pdf.swift:\(line))") }
}

@main
struct Main {
    static func main() {
        PythonHelper.shared.configure(libDir:
            URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent("pylib").path)
        let fence = "```cpp\n#include <iostream>\nint main(){ std::cout << \"hello\"; }\n```\n"
        if RichText.available {
            let hl = RichText.pandocHTML(fence, highlight: true) ?? ""
            check(hl.contains("sourceCode cpp"), "prose fence is a sourceCode block: \(hl)")
            check(hl.contains("class=\"dt\""), "int is a type token: \(hl)")
            let plain = RichText.pandocHTML(fence) ?? ""
            check(plain.contains("<pre class=\"cpp\"><code>") && !plain.contains("<span"), "pastes stay unhighlighted: \(plain)")
        } else { print("  (pandoc missing: highlight checks skipped)") }

        print("prose: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }
}
