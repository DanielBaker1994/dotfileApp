import Foundation

enum DocTemplates {
    static let builtin = ["tokyo-night", "paper", "executive", "terminal", "catppuccin-mocha", "catppuccin-latte", "dracula", "nord", "gruvbox-dark", "gruvbox-light", "solarized-dark", "solarized-light", "rose-pine", "rose-pine-dawn"]

    static func fromCSS(_ css: String) -> [String] {
        guard let re = try? NSRegularExpression(pattern: #":root:has\(\.([A-Za-z0-9_-]+)\)\s*\{[^}]*--p-bg"#) else { return [] }
        var seen = Set<String>(), out: [String] = []
        for m in re.matches(in: css, range: NSRange(css.startIndex..., in: css)) {
            guard let r = Range(m.range(at: 1), in: css) else { continue }
            let n = String(css[r])
            if n != "doc", seen.insert(n).inserted { out.append(n) }
        }
        return out
    }

    static func names(_ configured: String?, css: String? = nil) -> [String] {
        let list = (configured ?? "").split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        if !list.isEmpty { return list }
        let found = css.map(fromCSS) ?? []
        return found.isEmpty ? builtin : found
    }

    static func parse(_ line: String) -> (template: String, foot: String)? {
        let t = line.trimmingCharacters(in: .whitespaces)
        guard t.hasPrefix("<div"), t.hasSuffix("</div>"),
              let cls = capture(#"class="([^"]*)""#, t) else { return nil }
        let words = cls.split(separator: " ").map(String.init)
        guard words.first == "doc" else { return nil }
        return (words.dropFirst().first ?? "", capture(#"data-foot="([^"]*)""#, t) ?? "")
    }

    static func current(in text: String) -> String? {
        parse(text.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? "")?.template
    }

    static func marker(_ template: String, foot: String) -> String {
        "<div class=\"doc \(template)\"" + (foot.isEmpty ? "" : " data-foot=\"\(foot)\"") + "></div>"
    }

    static func edit(_ text: String, template: String?) -> (remove: Int, insert: [String]) {
        let lines = text.components(separatedBy: "\n")
        let first = lines.first ?? ""
        let old = parse(first)
        let blankAfter = lines.count > 1 && lines[1].trimmingCharacters(in: .whitespaces).isEmpty
        switch (old, template) {
        case (nil, nil): return (0, [])
        case (nil, let t?): return (0, [marker(t, foot: ""), ""])
        case (_?, nil): return (blankAfter ? 2 : 1, [])
        case (let o?, let t?): return (1, [marker(t, foot: o.foot)])
        }
    }

    static func apply(_ text: String, template: String?) -> String {
        let e = edit(text, template: template)
        var lines = text.components(separatedBy: "\n")
        lines.replaceSubrange(0..<min(e.remove, lines.count), with: e.insert)
        return lines.joined(separator: "\n")
    }

    private static func capture(_ pattern: String, _ s: String) -> String? {
        guard let re = try? NSRegularExpression(pattern: pattern),
              let m = re.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)),
              let r = Range(m.range(at: 1), in: s) else { return nil }
        return String(s[r])
    }
}
