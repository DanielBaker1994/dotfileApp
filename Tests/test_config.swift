// sources: PythonHelper.swift ConfigText.swift
import Foundation

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition { passed += 1 } else { failed += 1; print("  FAIL: \(message) (test_config.swift:\(line))") }
}

func checkEqual<T: Equatable>(_ actual: T, _ expected: T, _ message: String, line: Int = #line) {
    check(actual == expected, "\(message): got \(String(reflecting: actual)), want \(String(reflecting: expected))",
          line: line)
}

func testTri() {
    print("tri:")
    for s in ["true", "yes", "1", "on", "TRUE", "On"] { check(tri(s) == true, "'\(s)' → true") }
    for s in ["false", "no", "0", "off", "No"] { check(tri(s) == false, "'\(s)' → false") }
    for s: String? in [nil, "", "maybe", "2"] { check(tri(s) == nil, "\(String(reflecting: s)) → nil") }
}

func testResolveBinary() {
    print("resolveBinary:")
    checkEqual(resolveBinary("/bin/sh"), "/bin/sh", "absolute executable path")
    check(resolveBinary("/etc/hosts") == nil, "absolute non-executable path")
    check(resolveBinary("ls")?.hasSuffix("/ls") == true, "'ls' found on PATH")
    check(resolveBinary("definitely-not-a-real-binary-xyz") == nil, "unknown name")
}

@main
struct ConfigHelpers {
    static func main() {
        testTri()
        testResolveBinary()
        print("\n=== Results: \(passed) passed, \(failed) failed ===")
        exit(failed == 0 ? 0 : 1)
    }
}
