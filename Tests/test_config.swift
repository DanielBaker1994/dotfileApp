// sources: ConfigText.swift
// commands.toml as text (ConfigText.swift): the one-line TOML codec, the
// section scanner, the writer's edits (configSetting), tri, resolveBinary.
// Usage: bin/run-tests.sh config

import Foundation

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL: \(message) (test_config.swift:\(line))")
    }
}

func checkEqual<T: Equatable>(_ actual: T, _ expected: T, _ message: String, line: Int = #line) {
    check(actual == expected, "\(message): got \(String(reflecting: actual)), want \(String(reflecting: expected))",
          line: line)
}

// configEntry as a comparable pair (nil = not an entry)
func entry(_ line: String) -> [String]? {
    configEntry(line).map { [$0.key, $0.value] }
}

// configSetting over text, joined back (what saveConfigValues writes)
func setting(_ text: String, _ section: String, _ kv: [(String, String?)]) -> String {
    configSetting(configLines(text), section: section, kv).joined(separator: "\n")
}

func testEntries() {
    print("configEntry:")
    checkEqual(entry("vim-mode = true"), ["vim-mode", "true"], "bare bool")
    checkEqual(entry("  width = 780  "), ["width", "780"], "surrounding spaces")
    checkEqual(entry("shell = \"/bin/zsh\""), ["shell", "/bin/zsh"], "basic string")
    checkEqual(entry("re = '\\d+ \"x\"'"), ["re", "\\d+ \"x\""], "literal string: no escapes")
    checkEqual(entry("t = \"a\\tb\\n\\\"q\\\" \\u00e9\""), ["t", "a\tb\n\"q\" é"], "basic escapes")
    checkEqual(entry("paths = [\"~/a.md\", '~/b.md', 3]"), ["paths", "~/a.md, ~/b.md, 3"], "array → one comma string")
    checkEqual(entry("n = 42 # answer"), ["n", "42"], "comment after a scalar dropped")
    checkEqual(entry("s = \"x\" # note"), ["s", "x"], "comment after a string dropped")
    checkEqual(entry("font = Menlo # legacy"), ["font", "Menlo # legacy"], "legacy unquoted value kept whole")
    checkEqual(entry("\"view: keys\" = \"what\""), ["view: keys", "what"], "quoted key")
    checkEqual(entry("url = \"a=b\""), ["url", "a=b"], "'=' inside the value")
    checkEqual(entry("empty ="), ["empty", ""], "empty value")
    check(entry("# comment") == nil, "comment line")
    check(entry("") == nil, "blank line")
    check(entry("[app]") == nil, "section header")
    check(entry("no equals sign") == nil, "no '='")
}

func testLines() {
    print("configLine:")
    checkEqual(configLine("float", "true"), "float = true", "bool stays bare")
    checkEqual(configLine("width", "-12.5"), "width = -12.5", "number stays bare")
    checkEqual(configLine("width", "007"), "width = \"007\"", "leading-zero number is a string")
    checkEqual(configLine("shell", "/bin/zsh"), "shell = \"/bin/zsh\"", "string quoted")
    checkEqual(configLine("view: keys", "x"), "\"view: keys\" = \"x\"", "non-bare key quoted")
    // everything written must read back as written
    let tricky = ["", "plain", "with \"quotes\"", "back\\slash", "tab\tand\nnewline",
                  "# not a comment", "a, b, c", "é ü 🐙", "bell\u{7}", "[not an array]", "true", "3.14"]
    for v in tricky {
        checkEqual(entry(configLine("k", v)), ["k", v], "round trip \(String(reflecting: v))")
    }
    checkEqual(entry(configLine("a b = c", "v")), ["a b = c", "v"], "round trip of a key with '='")
}

func testSections() {
    print("sections:")
    checkEqual(configSectionHeader("[app]"), "app", "header")
    checkEqual(configSectionHeader("  [ notes ]  "), "notes", "spaces trimmed (TOML allows them)")
    check(configSectionHeader("key = [a]") == nil, "an array value is not a header")
    check(configSectionHeader("[unclosed") == nil, "unclosed")
    checkEqual(configLines("a\n\nb\n"), ["a", "", "b", ""], "lines keep empties (join restores the text)")

    let lines = configLines("""
        # top
        [app]
        float = true
        # comment
        shell = "/bin/zsh"
        [notes]
        paths = "~/a.md"
        [ app ]
        float = false
        """)
    let app = configSectionEntries(lines, "app")
    checkEqual(app.map(\.index), [2, 4, 8], "indices of [app] entries, both [app] blocks")
    checkEqual(app.map(\.key), ["float", "shell", "float"], "keys in file order")
    checkEqual(app.map(\.value), ["true", "/bin/zsh", "false"], "decoded values")
    checkEqual(configSectionEntries(lines, "notes").map(\.key), ["paths"], "other section")
    check(configSectionEntries(lines, "missing").isEmpty, "missing section")
}

func testSetting() {
    print("configSetting:")
    let base = """
        [app]
        shell = "/bin/bash"

        # notes window
        [notes]
        enabled = true
        vim-mode = false

        """
    checkEqual(setting(base, "notes", [("vim-mode", "true")]),
               base.replacingOccurrences(of: "vim-mode = false", with: "vim-mode = true"),
               "update in place")
    checkEqual(setting(base, "app", [("float", "true")]),
               base.replacingOccurrences(of: "shell = \"/bin/bash\"", with: "shell = \"/bin/bash\"\nfloat = true"),
               "new key after the section's last entry")
    checkEqual(setting("[app]\n", "app", [("k", "v")]), "[app]\nk = \"v\"\n", "new key under an empty section")
    checkEqual(setting(base, "runtime", [("test", "value")]), base + "\n\n[runtime]\ntest = \"value\"",
               "missing section appended")
    checkEqual(setting(base, "app", [("shell", nil)]),
               base.replacingOccurrences(of: "shell = \"/bin/bash\"\n", with: ""),
               "remove keeps everything else, trailing newline too")
    checkEqual(setting(base, "app", [("absent", nil)]), base, "removing an absent key changes nothing")
    checkEqual(setting(base, "notes", [("enabled", "false"), ("vim-mode", nil), ("font", "Menlo")]),
               base.replacingOccurrences(of: "enabled = true\nvim-mode = false", with: "enabled = false\nfont = \"Menlo\""),
               "several keys in one edit")
    checkEqual(setting("[a]\nx = 1\nx = 2\n", "a", [("x", "3")]), "[a]\nx = 1\nx = 3\n",
               "a duplicated key: the last one (the one that counts) is edited")
    checkEqual(setting("[a]\nk = 1\n[b]\n[a]\n", "a", [("n", "2")]), "[a]\nk = 1\n[b]\n[a]\nn = 2\n",
               "a section split in two: new key goes in the later block")
    checkEqual(setting("[ a ]\nk = 1\n", "a", [("k", "2")]), "[ a ]\nk = 2\n", "spaced header found")
    checkEqual(setting("[b]\nk = 1\n", "a", [("k", "2")]), "[b]\nk = 1\n\n\n[a]\nk = 2",
               "same key in another section is not touched")
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
struct ConfigTests {
    static func main() {
        testEntries()
        testLines()
        testSections()
        testSetting()
        testTri()
        testResolveBinary()
        print("\n=== Results: \(passed) passed, \(failed) failed ===")
        exit(failed == 0 ? 0 : 1)
    }
}
