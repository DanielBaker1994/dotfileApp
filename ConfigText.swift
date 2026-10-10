import Foundation

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

func configSectionHeader(_ line: String) -> String? {
    let s = line.trimmingCharacters(in: .whitespaces)
    guard s.hasPrefix("[") && s.hasSuffix("]") else { return nil }
    return String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
}

func configLines(_ text: String) -> [String] {
    text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
}

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

func configSetting(_ lines: [String], section: String, _ kv: [(String, String?)]) -> [String] {
    var lines = lines
    for (key, value) in kv {
        let entries = configSectionEntries(lines, section)
        let found = entries.last(where: { $0.key == key })?.index
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

func tri(_ s: String?) -> Bool? {
    switch s?.lowercased() {
    case "true", "yes", "1", "on": return true
    case "false", "no", "0", "off": return false
    default: return nil
    }
}

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
