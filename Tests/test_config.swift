#!/usr/bin/env swift
// Minimal assertion helper for standalone Swift tests
// Usage: swift test_config.swift

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

// MARK: - Config parsing tests

func testParseKeyValue() {
    print("Config key-value parsing:")

    let line = "vim-mode = true"
    guard let eq = line.firstIndex(of: "=") else {
        assert(false, "should find '=' in key-value line")
        return
    }
    let key = String(line[..<eq]).trimmingCharacters(in: .whitespaces)
    let val = String(line[line.index(after: eq)...]).trimmingCharacters(in: .whitespaces)
    assertEquals(key, "vim-mode", "key parsed correctly")
    assertEquals(val, "true", "value parsed correctly")
}

func testTriFunction() {
    print("Boolean tri-state parsing:")

    func tri(_ s: String?) -> Bool? {
        switch s?.lowercased() {
        case "true", "yes", "1", "on": return true
        case "false", "no", "0", "off": return false
        default: return nil
        }
    }

    assert(tri("true") == true, "'true' → true")
    assert(tri("yes") == true, "'yes' → true")
    assert(tri("1") == true, "'1' → true")
    assert(tri("on") == true, "'on' → true")
    assert(tri("false") == false, "'false' → false")
    assert(tri("no") == false, "'no' → false")
    assert(tri("0") == false, "'0' → false")
    assert(tri("off") == false, "'off' → false")
    assert(tri(nil) == nil, "nil → nil")
    assert(tri("maybe") == nil, "'maybe' → nil")
}

func testFindSection() {
    print("Section finding in config:")

    let config = """
    [app]
    shell = /bin/bash
    hide-on-focus-loss = false

    [notes]
    enabled = true
    vim-mode = true
    vim-bin = nvim
    """

    // Find [notes] section
    var inNotes = false
    var notesVars: [String: String] = [:]
    for line in config.split(separator: "\n") {
        let s = line.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("[") && s.hasSuffix("]") {
            inNotes = s == "[notes]"
            continue
        }
        guard inNotes, let eq = s.firstIndex(of: "=") else { continue }
        let k = String(s[..<eq]).trimmingCharacters(in: .whitespaces)
        let v = String(s[s.index(after: eq)...]).trimmingCharacters(in: .whitespaces)
        notesVars[k] = v
    }

    assertEquals(notesVars["vim-mode"], "true", "vim-mode found in [notes]")
    assertEquals(notesVars["vim-bin"], "nvim", "vim-bin found in [notes]")
    assertEquals(notesVars["enabled"], "true", "enabled found in [notes]")
}

func testSaveConfigValue() {
    print("Config value saving:")

    // Simulate the save logic
    func updateConfig(_ content: String, section: String, key: String, value: String) -> String {
        var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        var inTarget = false
        var keyFound = false
        var insertAfter = -1

        for i in 0..<lines.count {
            let s = lines[i].trimmingCharacters(in: .whitespaces)
            if s.hasPrefix("[") && s.hasSuffix("]") {
                let name = String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
                inTarget = name == section
                if inTarget { insertAfter = i }
                continue
            }
            guard inTarget, let eq = s.firstIndex(of: "=") else { continue }
            let k = String(s[..<eq]).trimmingCharacters(in: .whitespaces)
            if k == key {
                lines[i] = "\(key) = \(value)"
                keyFound = true
                break
            }
            insertAfter = i
        }

        if !keyFound, insertAfter >= 0 {
            lines.insert("\(key) = \(value)", at: insertAfter + 1)
        } else if !keyFound {
            lines.append("")
            lines.append("[\(section)]")
            lines.append("\(key) = \(value)")
        }

        return lines.joined(separator: "\n")
    }

    let original = """
    [app]
    shell = /bin/bash

    [notes]
    enabled = true
    vim-mode = false
    """

    // Test 1: Update existing key
    let updated = updateConfig(original, section: "notes", key: "vim-mode", value: "true")
    assert(updated.contains("vim-mode = true"), "existing key updated")

    // Test 2: Add new key to existing section
    let withNewKey = updateConfig(original, section: "notes", key: "vim-bin", value: "nvim")
    assert(withNewKey.contains("vim-bin = nvim"), "new key added to existing section")
    assert(withNewKey.contains("vim-mode = false"), "existing key preserved")

    // Test 3: Add new section
    let withNewSection = updateConfig(original, section: "runtime", key: "test", value: "value")
    assert(withNewSection.contains("[runtime]"), "new section added")
    assert(withNewSection.contains("test = value"), "key in new section")
}

func testRemoveConfigValue() {
    print("Config value removal:")

    func removeKey(_ content: String, section: String, key: String) -> String {
        var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        var inTarget = false
        var toRemove: [Int] = []

        for i in 0..<lines.count {
            let s = lines[i].trimmingCharacters(in: .whitespaces)
            if s.hasPrefix("[") && s.hasSuffix("]") {
                let name = String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
                inTarget = name == section
                continue
            }
            guard inTarget, let eq = s.firstIndex(of: "=") else { continue }
            let k = String(s[..<eq]).trimmingCharacters(in: .whitespaces)
            if k == key {
                toRemove.append(i)
                break
            }
        }

        for idx in toRemove.sorted(by: >) {
            lines.remove(at: idx)
        }

        while lines.last?.isEmpty ?? false, lines.count > 1 {
            lines.removeLast()
        }

        return lines.joined(separator: "\n")
    }

    let original = """
    [app]
    shell = /bin/bash
    hide-on-focus-loss = false

    [notes]
    enabled = true
    vim-mode = true
    vim-bin = nvim
    """

    let removed = removeKey(original, section: "app", key: "hide-on-focus-loss")
    assert(!removed.contains("hide-on-focus-loss"), "key removed from [app]")
    assert(removed.contains("shell = /bin/bash"), "other key preserved")
    assert(removed.contains("[notes]"), "other section preserved")
    assert(removed.contains("vim-mode = true"), "notes section untouched")
}

func testResolveBinary() {
    print("Binary resolution:")

    // Test with absolute path
    func resolveBinary(_ name: String) -> String? {
        if name.hasPrefix("/") {
            return FileManager.default.isExecutableFile(atPath: name) ? name : nil
        }
        let paths = (ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin")
            .split(separator: ":").map(String.init)
        for dir in paths {
            let fullPath = dir + "/" + name
            if FileManager.default.isExecutableFile(atPath: fullPath) {
                return fullPath
            }
        }
        return nil
    }

    // /bin/bash should exist
    let bash = resolveBinary("/bin/bash")
    assert(bash == "/bin/bash", "absolute path resolved when executable")

    // 'ls' should be in PATH
    let ls = resolveBinary("ls")
    assert(ls != nil, "'ls' found in PATH")
    assert(ls?.hasSuffix("/ls") ?? false, "'ls' path ends with /ls")

    // non-existent binary
    let fake = resolveBinary("definitely-not-a-real-binary-xyz")
    assert(fake == nil, "non-existent binary returns nil")
}

// MARK: - Run tests

print("=== Workspace Switcher Config Tests ===\n")
testParseKeyValue()
print()
testTriFunction()
print()
testFindSection()
print()
testSaveConfigValue()
print()
testRemoveConfigValue()
print()
testResolveBinary()

print("\n=== Results: \(passed) passed, \(failed) failed ===")
exit(failed > 0 ? 1 : 0)
