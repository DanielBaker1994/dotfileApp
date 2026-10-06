// ConfigText.swift — commands.toml as TEXT: the one-line TOML codec, the
// section scanner and the pure edits every reader / writer goes through.
// Foundation only (no app state, no file IO), so Tests/test_config.swift
// compiles it on its own. Reading / validating / writing the file itself
// (readConfigText, writeConfigText, validateConfig) stays in
// kitchen_sink.swift.

import Foundation

// commands.toml is TOML, read line by line: `[section]` headers, then one
// `key = value` per line. Keys are bare or "quoted"; values are "basic" /
// 'literal' strings, bare true/false/numbers, or a one-line [array] (read as
// "a, b"). Every value reaches the app as a String — lists stay comma-
// separated inside one string. Old unquoted values still read as written.
// Mirrored in python: jira_config.config_entry / config_line.
func configEntry(_ line: String) -> (key: String, value: String)? {
    let s = line.trimmingCharacters(in: .whitespaces)
    guard !s.isEmpty, !s.hasPrefix("#"), !s.hasPrefix("[") else { return nil }
    var rest = Substring(s)
    let key: String
    if s.hasPrefix("\"") || s.hasPrefix("'") {
        guard let (k, after) = tomlScanString(rest) else { return nil }
        rest = after.drop(while: { $0 == " " || $0 == "\t" })
        guard rest.first == "=" else { return nil }
        key = k
        rest = rest.dropFirst()
    } else {
        guard let eq = rest.firstIndex(of: "=") else { return nil }
        key = rest[..<eq].trimmingCharacters(in: .whitespaces)
        rest = rest[rest.index(after: eq)...]
    }
    return (key, tomlValue(rest.trimmingCharacters(in: .whitespaces)))
}

// the name of a `[section]` header line (inner spaces trimmed), else nil
func configSectionHeader(_ line: String) -> String? {
    let s = line.trimmingCharacters(in: .whitespaces)
    guard s.hasPrefix("[") && s.hasSuffix("]") else { return nil }
    return String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
}

// commands.toml text as editable lines (empty lines kept, so a join restores it)
func configLines(_ text: String) -> [String] {
    text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
}

// every `key = value` entry of [section] in file order, with its index in `lines`
func configSectionEntries(_ lines: [String], _ section: String)
    -> [(index: Int, key: String, value: String)] {
    var out: [(index: Int, key: String, value: String)] = []
    var inSection = false
    for (i, line) in lines.enumerated() {
        if let name = configSectionHeader(line) {
            inSection = name == section
        } else if inSection, let e = configEntry(line) {
            out.append((i, e.key, e.value))
        }
    }
    return out
}

// `key = value` as a TOML line (strings quoted, bools/numbers bare)
func configLine(_ key: String, _ value: String) -> String {
    let bare = !key.isEmpty && key.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_") }
    return (bare ? key : tomlQuote(key)) + " = "
        + (tomlBareScalar(value) ? value : tomlQuote(value))
}

private func tomlBareScalar(_ v: String) -> Bool {
    v == "true" || v == "false"
        || v.range(of: #"^-?(0|[1-9][0-9]*)(\.[0-9]+)?$"#, options: .regularExpression) != nil
}

private func tomlQuote(_ v: String) -> String {
    var out = "\""
    for u in v.unicodeScalars {
        switch u {
        case "\"": out += "\\\""
        case "\\": out += "\\\\"
        case "\n": out += "\\n"
        case "\t": out += "\\t"
        case "\r": out += "\\r"
        case _ where u.value < 0x20 || u.value == 0x7F: out += String(format: "\\u%04X", u.value)
        default: out.unicodeScalars.append(u)
        }
    }
    return out + "\""
}

// a "basic" or 'literal' string at the start of `s` → (text, what follows)
private func tomlScanString(_ s: Substring) -> (String, Substring)? {
    guard let q = s.first, q == "\"" || q == "'" else { return nil }
    var i = s.index(after: s.startIndex)
    if q == "'" {
        guard let end = s[i...].firstIndex(of: "'") else { return nil }
        return (String(s[i..<end]), s[s.index(after: end)...])
    }
    var out = ""
    while i < s.endIndex {
        let c = s[i]
        if c == "\"" { return (out, s[s.index(after: i)...]) }
        if c == "\\" {
            i = s.index(after: i)
            guard i < s.endIndex else { return nil }
            switch s[i] {
            case "n": out += "\n"
            case "t": out += "\t"
            case "r": out += "\r"
            case "b": out += "\u{8}"
            case "f": out += "\u{C}"
            case "\"": out += "\""
            case "\\": out += "\\"
            case "u", "U":
                let n = s[i] == "u" ? 4 : 8
                guard let end = s.index(i, offsetBy: n, limitedBy: s.index(before: s.endIndex)),
                      let v = UInt32(s[s.index(after: i)...end], radix: 16),
                      let u = Unicode.Scalar(v) else { return nil }
                out.unicodeScalars.append(u)
                i = end
            default: return nil
            }
        } else {
            out.append(c)
        }
        i = s.index(after: i)
    }
    return nil
}

// the text of a value: strings unquoted, [arrays] joined ", ", bare values
// as written (a trailing `# comment` dropped only after a TOML scalar)
private func tomlValue(_ raw: String) -> String {
    func onlyComment(_ rest: Substring) -> Bool {
        let t = rest.trimmingCharacters(in: .whitespaces)
        return t.isEmpty || t.hasPrefix("#")
    }
    if raw.hasPrefix("\"") || raw.hasPrefix("'") {
        if let (v, after) = tomlScanString(Substring(raw)), onlyComment(after) { return v }
        return raw
    }
    if raw.hasPrefix("[") {
        var items: [String] = []
        var rest = Substring(raw).dropFirst()
        while true {
            rest = rest.drop(while: { $0 == " " || $0 == "\t" })
            if rest.first == "]" { return onlyComment(rest.dropFirst()) ? items.joined(separator: ", ") : raw }
            if let (v, after) = tomlScanString(rest) {
                items.append(v)
                rest = after
            } else {
                let end = rest.firstIndex(where: { $0 == "," || $0 == "]" }) ?? rest.endIndex
                let v = rest[..<end].trimmingCharacters(in: .whitespaces)
                guard tomlBareScalar(v) else { return raw }
                items.append(v)
                rest = rest[end...]
            }
            rest = rest.drop(while: { $0 == " " || $0 == "\t" })
            if rest.first == "," { rest = rest.dropFirst() } else if rest.first != "]" { return raw }
        }
    }
    if let hash = raw.range(of: " #") {
        let head = raw[..<hash.lowerBound].trimmingCharacters(in: .whitespaces)
        if tomlBareScalar(head) { return head }
    }
    return raw
}

// `lines` with several keys of one [section] set (or, for a nil value,
// removed): a key is updated where it is, else added after the section's
// last entry (or under a new section at the end). A key listed twice is
// edited where it takes effect: the last one (validateConfig).
func configSetting(_ lines: [String], section: String, _ kv: [(String, String?)]) -> [String] {
    var lines = lines
    for (key, value) in kv {
        let entries = configSectionEntries(lines, section)
        let found = entries.last(where: { $0.key == key })?.index
        // the new line's slot: after the section's last entry, else its header
        let header = lines.indices.last(where: { configSectionHeader(lines[$0]) == section })
        let lastInSection = entries.last.map { max($0.index, header ?? -1) } ?? header
        switch (found, value) {
        case (let i?, let v?): lines[i] = configLine(key, v)
        case (let i?, nil): lines.remove(at: i)
        case (nil, let v?):
            if let at = lastInSection {
                lines.insert(configLine(key, v), at: at + 1)
            } else {
                lines += ["", "[\(section)]", configLine(key, v)]
            }
        case (nil, nil): break
        }
    }
    return lines
}

// "true/yes/1/on" | "false/no/0/off" | anything else = nil (unset)
func tri(_ s: String?) -> Bool? {
    switch s?.lowercased() {
    case "true", "yes", "1", "on": return true
    case "false", "no", "0", "off": return false
    default: return nil
    }
}

// Resolve a binary by name: checks the PATH, returns the absolute path
// or nil if not found. If the input is already an absolute path, returns
// it directly if it exists.
func resolveBinary(_ name: String) -> String? {
    if name.hasPrefix("/") {
        return FileManager.default.isExecutableFile(atPath: name) ? name : nil
    }
    let paths = (ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin")
        .split(separator: ":").map(String.init)
    for dir in paths {
        let fullPath = dir + "/" + name
        if FileManager.default.isExecutableFile(atPath: fullPath) {
            return fullPath
        }
    }
    return nil
}
