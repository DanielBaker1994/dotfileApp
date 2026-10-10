import Foundation

struct ConfigDecodedLine {
    let index: Int
    let trimmed: String
    let header: String?
    let key: String?
    let value: String?
}

private let configCacheDir = NSHomeDirectory() + "/.cache/kitchen-sink"

private func configCacheKey(_ parts: String...) -> String {
    var hash: UInt64 = 14695981039346656037
    for byte in parts.joined(separator: "\u{1}").utf8 {
        hash = (hash ^ UInt64(byte)) &* 1099511628211
    }
    return String(format: "%016llx", hash)
}

private func configCacheRead(_ key: String) -> Any? {
    let path = configCacheDir + "/config-" + key + ".json"
    guard let data = FileManager.default.contents(atPath: path) else { return nil }
    return try? JSONSerialization.jsonObject(with: data)
}

private func configCacheWrite(_ key: String, _ json: Any) {
    try? FileManager.default.createDirectory(atPath: configCacheDir, withIntermediateDirectories: true)
    guard let data = try? JSONSerialization.data(withJSONObject: json) else { return }
    try? data.write(to: URL(fileURLWithPath: configCacheDir + "/config-" + key + ".json"),
                    options: .atomic)
}

func configDecodedLines(_ text: String) -> [ConfigDecodedLine] {
    let key = configCacheKey("decode", text)
    let raw: [[String: Any]]
    if let cached = configCacheRead(key) as? [[String: Any]] {
        raw = cached
    } else {
        guard case .success(let box) = PythonHelper.shared.callSync("config.decode", ["text": text]),
              let live = (box as? [String: Any])?["lines"] as? [[String: Any]] else { return [] }
        configCacheWrite(key, live)
        raw = live
    }
    return raw.map {
        ConfigDecodedLine(index: $0["index"] as? Int ?? 0,
                          trimmed: $0["trimmed"] as? String ?? "",
                          header: $0["header"] as? String,
                          key: $0["key"] as? String,
                          value: $0["value"] as? String)
    }
}

func configSectionEntries(_ lines: [String], _ section: String)
    -> [(index: Int, key: String, value: String)] {
    let text = lines.joined(separator: "\n")
    let key = configCacheKey("section", section, text)
    let raw: [[String: Any]]
    if let cached = configCacheRead(key) as? [[String: Any]] {
        raw = cached
    } else {
        guard case .success(let box) = PythonHelper.shared.callSync(
                "config.section_entries", ["text": text, "section": section]),
              let live = (box as? [String: Any])?["entries"] as? [[String: Any]] else { return [] }
        configCacheWrite(key, live)
        raw = live
    }
    return raw.compactMap {
        guard let i = $0["index"] as? Int, let k = $0["key"] as? String, let v = $0["value"] as? String
        else { return nil }
        return (i, k, v)
    }
}

func configLine(_ key: String, _ value: String) -> String? {
    guard case .success(let box) = PythonHelper.shared.callSync("config.line", ["key": key, "value": value]),
          let line = (box as? [String: Any])?["line"] as? String else { return nil }
    return line
}

func configSettingText(_ text: String, section: String, _ kv: [(String, String?)]) -> String? {
    let pairs: [[Any]] = kv.map { [$0.0, $0.1 ?? NSNull()] }
    guard case .success(let box) = PythonHelper.shared.callSync(
            "config.setting", ["text": text, "section": section, "kv": pairs]),
          let out = (box as? [String: Any])?["text"] as? String else { return nil }
    return out
}

func configLines(_ text: String) -> [String] {
    text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
}

private let triLock = NSLock()
private var triMemo: [String: Any] = [:]

func tri(_ s: String?) -> Bool? {
    // the grammar lives in pylib/config_text.py; memoised because config
    // loaders call this per key and the answer set is tiny
    let key = (s ?? "").lowercased()
    triLock.lock()
    if let hit = triMemo[key] { triLock.unlock(); return hit as? Bool }
    triLock.unlock()
    var value: Bool?
    if case .success(let box) = PythonHelper.shared.callSync(
            "config.tri", ["text": key], timeout: 30),
       let d = box as? [String: Any] {
        value = d["value"] as? Bool
    }
    triLock.lock()
    triMemo[key] = value ?? NSNull()
    triLock.unlock()
    return value
}

func resolveBinary(_ name: String) -> String? {
    guard case .success(let box) = PythonHelper.shared.callSync(
            "config.resolve_binary", ["name": name], timeout: 30),
          let d = box as? [String: Any] else { return nil }
    return d["path"] as? String
}
