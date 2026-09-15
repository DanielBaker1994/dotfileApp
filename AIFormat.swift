import AppKit

// MARK: - AI view: Markdown -> Outlook / Webex, and fitting text into fm
//
// Copy writes ONE pasteboard item: HTML (pandoc, every style inline because
// mail clients drop <style>), RTF, and the Markdown as text. Webex has no
// tables: for it a table becomes an aligned monospace block.

enum PasteTarget: Int {
    case outlook = 0, webex = 1
    var title: String { self == .outlook ? "Outlook" : "Webex" }
}

// the AI view's right pane: the diff, the Markdown, or a paste preview
enum PaneMode: String {
    case diff, markdown, outlook, webex
    var title: String { rawValue.prefix(1).uppercased() + rawValue.dropFirst() }
    var target: PasteTarget? { self == .outlook ? .outlook : self == .webex ? .webex : nil }
    static func all(diff: Bool) -> [PaneMode] { (diff ? [.diff] : []) + [.markdown, .outlook, .webex] }
}

enum RichText {
    static var pandocBin: String { (aiSetting("pandoc-bin", "/opt/homebrew/bin/pandoc") as NSString).expandingTildeInPath }
    static var available: Bool { FileManager.default.isExecutableFile(atPath: pandocBin) }

    // Webex can't show tables: each becomes an aligned block in a code fence
    static func tablesAsText(_ md: String) -> String {
        let lines = md.components(separatedBy: "\n")
        var out: [String] = []
        var i = 0
        var inFence = false
        while i < lines.count {
            let t = lines[i].trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") { inFence.toggle() }
            if !inFence, t.hasPrefix("|"), i + 1 < lines.count, isSeparator(lines[i + 1]) {
                var rows: [[String]] = [cells(lines[i])]
                var j = i + 2
                while j < lines.count, lines[j].trimmingCharacters(in: .whitespaces).hasPrefix("|") {
                    rows.append(cells(lines[j]))
                    j += 1
                }
                let n = rows.map(\.count).max() ?? 0
                let rows2 = rows.map { $0 + Array(repeating: "", count: n - $0.count) }
                let w = (0..<n).map { c in rows2.map { $0[c].count }.max() ?? 0 }
                func line(_ r: [String]) -> String {
                    r.enumerated().map { $0.element.padding(toLength: w[$0.offset], withPad: " ", startingAt: 0) }
                        .joined(separator: "  ").trimmingCharacters(in: .whitespaces)
                }
                out.append("```")
                out.append(line(rows2[0]))
                out.append(w.map { String(repeating: "-", count: max(1, $0)) }.joined(separator: "  "))
                out += rows2.dropFirst().map(line)
                out.append("```")
                i = j
                continue
            }
            out.append(lines[i])
            i += 1
        }
        return out.joined(separator: "\n")
    }

    private static func isSeparator(_ s: String) -> Bool {
        s.range(of: #"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$"#, options: .regularExpression) != nil
    }

    // a table row's cells, inline markup stripped (they land in plain text)
    private static func cells(_ row: String) -> [String] {
        var s = row.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("|") { s.removeFirst() }
        if s.hasSuffix("|") && !s.hasSuffix("\\|") { s.removeLast() }
        return s.replacingOccurrences(of: "\\|", with: "\u{1}").components(separatedBy: "|").map {
            $0.replacingOccurrences(of: "\u{1}", with: "|")
                .replacingOccurrences(of: #"\*\*|__|`"#, with: "", options: .regularExpression)
                .trimmingCharacters(in: .whitespaces)
        }
    }

    // the Markdown that target gets
    static func markdown(_ md: String, for t: PasteTarget) -> String {
        t == .webex ? tablesAsText(md) : md
    }

    // pandoc: gfm -> an HTML fragment (nil = no pandoc / it failed)
    static func pandocHTML(_ md: String) -> String? {
        guard available else { return nil }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: pandocBin)
        p.arguments = ["-f", "gfm", "-t", "html", "--syntax-highlighting=none", "--wrap=none"]
        let inP = Pipe(), outP = Pipe()
        p.standardInput = inP
        p.standardOutput = outP
        p.standardError = FileHandle.nullDevice
        do { try p.run() } catch { return nil }
        let data = Data(md.utf8)
        DispatchQueue.global(qos: .userInitiated).async {
            inP.fileHandleForWriting.write(data)
            try? inP.fileHandleForWriting.close()
        }
        let out = outP.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return p.terminationStatus == 0 ? String(decoding: out, as: UTF8.self) : nil
    }

    // every style inline, in the target's own type
    static func styled(_ fragment: String, for t: PasteTarget) -> String {
        let font = t == .outlook ? "Aptos,Calibri,'Segoe UI',Helvetica,Arial,sans-serif"
                                 : "-apple-system,'Segoe UI',Helvetica,Arial,sans-serif"
        let size = t == .outlook ? "11pt" : "10.5pt"
        let mono = "Menlo,Consolas,'Courier New',monospace"
        let cell = "border:1px solid #bfbfbf;padding:4px 10px;vertical-align:top;"
        var h = fragment
        func sub(_ pattern: String, _ with: String) {
            h = h.replacingOccurrences(of: pattern, with: with, options: .regularExpression)
        }
        sub(#"<table[^>]*>"#, "<table style=\"border-collapse:collapse;margin:6px 0 10px 0;font-family:\(font);font-size:\(size)\">")
        sub(#"<th( style=\"([^\"]*)\")?>"#, "<th style=\"\(cell)background:#f2f2f2;font-weight:bold;text-align:left;$2\">")
        sub(#"<td( style=\"([^\"]*)\")?>"#, "<td style=\"\(cell)$2\">")
        // code blocks first (their <code> gets attributes), then inline code
        sub(#"<pre[^>]*>\s*<code[^>]*>"#, "<pre style=\"background:#f6f8fa;border:1px solid #d0d7de;border-radius:4px;"
            + "padding:8px 10px;margin:6px 0 10px 0;white-space:pre-wrap;font-family:\(mono);font-size:9.5pt;"
            + "color:#1f2328\"><code style=\"font-family:\(mono);font-size:9.5pt\">")
        sub(#"<code>"#, "<code style=\"font-family:\(mono);font-size:9.5pt;background:#f0f1f3;padding:1px 4px;"
            + "border-radius:3px;color:#1f2328\">")
        sub(#"<p>"#, "<p style=\"margin:0 0 8px 0\">")
        sub(#"<(ul|ol)>"#, "<$1 style=\"margin:0 0 8px 0;padding-left:22px\">")
        sub(#"<blockquote>"#, "<blockquote style=\"margin:0 0 8px 0;padding-left:10px;border-left:3px solid #c8c8c8;color:#555\">")
        sub(#"<h1([^>]*)>"#, "<h1$1 style=\"font-size:16pt;margin:10px 0 6px 0\">")
        sub(#"<h2([^>]*)>"#, "<h2$1 style=\"font-size:14pt;margin:10px 0 6px 0\">")
        sub(#"<h([3-6])([^>]*)>"#, "<h$1$2 style=\"font-size:12pt;margin:8px 0 4px 0\">")
        return "<div style=\"font-family:\(font);font-size:\(size);color:#1f1f1f;line-height:1.35\">\(h)</div>"
    }

    // the fragment the pasteboard (and the preview) get for a target
    static func html(_ md: String, for t: PasteTarget) -> String? {
        pandocHTML(markdown(md, for: t)).map { styled($0, for: t) }
    }

    static func document(_ fragment: String) -> String {
        "<html><head><meta charset=\"utf-8\"></head><body>\(fragment)</body></html>"
    }

    // RTF from the HTML (main thread: AppKit's HTML importer is WebKit)
    static func rtf(_ fragment: String) -> Data? {
        guard let a = try? NSAttributedString(data: Data(document(fragment).utf8),
                                              options: [.documentType: NSAttributedString.DocumentType.html,
                                                        .characterEncoding: String.Encoding.utf8.rawValue],
                                              documentAttributes: nil) else { return nil }
        return try? a.data(from: NSRange(location: 0, length: a.length),
                           documentAttributes: [.documentType: NSAttributedString.DocumentType.rtf])
    }

    // ONE pasteboard item: HTML + RTF + the Markdown as text
    static func copy(markdown md: String, fragment: String?, for t: PasteTarget) {
        let pb = NSPasteboard.general
        pb.clearContents()
        let item = NSPasteboardItem()
        if let f = fragment {
            item.setString(document(f), forType: .html)
            if let r = rtf(f) { item.setData(r, forType: .rtf) }
        }
        item.setString(markdown(md, for: t), forType: .string)
        pb.writeObjects([item])
    }
}

// code the model must not touch: fenced blocks + `inline` -> [[CODEn]]
struct CodeGuard {
    var text: String
    var codes: [String] = []

    static func token(_ n: Int) -> String { "[[CODE\(n)]]" }
    static let instruction = "Tokens like [[CODE1]] stand for code: keep every one exactly as written, in place."

    init(_ s: String) {
        var lines: [String] = []
        var block: [String]?
        var fence = ""
        var codes: [String] = []
        for line in s.components(separatedBy: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if block != nil {
                block!.append(line)
                if t.hasPrefix(fence) && t.trimmingCharacters(in: CharacterSet(charactersIn: String(fence.prefix(1)) + " ")).isEmpty {
                    codes.append(block!.joined(separator: "\n"))
                    lines.append(Self.token(codes.count))
                    block = nil
                }
            } else if t.hasPrefix("```") || t.hasPrefix("~~~") {
                fence = String(t.prefix(while: { $0 == t.first }))
                block = [line]
            } else {
                // inline spans, left to right
                var out = ""
                var rest = Substring(line)
                while let r = rest.range(of: #"`[^`\n]+`"#, options: .regularExpression) {
                    out += rest[..<r.lowerBound]
                    codes.append(String(rest[r]))
                    out += Self.token(codes.count)
                    rest = rest[r.upperBound...]
                }
                lines.append(out + rest)
            }
        }
        if let b = block { lines += b }     // an unclosed fence stays as text
        text = lines.joined(separator: "\n")
        self.codes = codes
    }

    // put the code back; `missing` = tokens the model dropped
    func restore(_ s: String) -> (text: String, missing: Int) {
        var out = s
        var missing = 0
        for (i, code) in codes.enumerated() {
            let tok = Self.token(i + 1)
            guard out.contains(tok) else { missing += 1; continue }
            // a model that wraps the token in backticks: the code has its own
            out = out.replacingOccurrences(of: "`" + tok + "`", with: code)
                .replacingOccurrences(of: tok, with: code)
        }
        return (out, missing)
    }
}

// fm's window: instructions + input + an answer about as long as the input
enum TokenBudget {
    static var context: Int { max(512, Int(aiNumber("context-tokens", 4096))) }
    // rough count for splitting (the footer shows fm's real count)
    static func estimate(_ s: String) -> Int { Int((Double(s.utf8.count) / 3.2).rounded(.up)) }
    // the room one part's input may take
    static func partBudget(instructions: String) -> Int {
        max(200, (Int(Double(context) * 0.9) - estimate(instructions) - 64) / 2)
    }

    // split at blank lines into parts that fit (a paragraph, table or code
    // token never splits; one that's too big alone goes as its own part)
    static func parts(_ s: String, budget: Int) -> [String] {
        guard estimate(s) > budget else { return [s] }
        var parts: [String] = []
        var cur = ""
        for para in s.components(separatedBy: "\n\n") {
            let next = cur.isEmpty ? para : cur + "\n\n" + para
            if estimate(next) > budget && !cur.isEmpty {
                parts.append(cur)
                cur = para
            } else {
                cur = next
            }
        }
        if !cur.isEmpty { parts.append(cur) }
        return parts
    }
}

// fm's small model follows one-line rules far better than hard-wrapped
// ones: the rule file stays wrapped for reading, the instructions it gets
// are reflowed (a wrapped line joins the line above; blank lines, bullets,
// numbers, headings, table rows and code blocks start their own)
enum Reflow {
    static func instructions(_ s: String) -> String {
        var out: [String] = []
        var inFence = false
        for raw in s.components(separatedBy: "\n") {
            let t = raw.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") { inFence.toggle(); out.append(raw); continue }
            let starts = t.isEmpty || inFence || t.hasPrefix("- ") || t.hasPrefix("* ") || t.hasPrefix("#")
                || t.hasPrefix("|") || t.range(of: #"^\d+[.)] "#, options: .regularExpression) != nil
            if !starts, let last = out.last, !last.trimmingCharacters(in: .whitespaces).isEmpty,
               !(last.trimmingCharacters(in: .whitespaces).hasPrefix("```")) {
                out[out.count - 1] = last + " " + t
            } else {
                out.append(inFence ? raw : t)
            }
        }
        return out.joined(separator: "\n")
    }
}

// what small models add around an answer
enum AnswerCleanup {
    // the whole answer wrapped in one ``` / ```markdown fence (often never
    // closed, which shifts every later fence): drop the wrapper. Runs on
    // the model's text BEFORE [[CODEn]] go back, so the user's code is safe.
    static func unwrapFence(_ s: String) -> String {
        var lines = s.components(separatedBy: "\n")
        guard let first = lines.firstIndex(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) else { return s }
        let open = lines[first].trimmingCharacters(in: .whitespaces).lowercased()
        guard ["```", "```markdown", "```md", "~~~"].contains(open) else { return s }
        lines.remove(at: first)
        let fences = lines.indices.filter {
            let t = lines[$0].trimmingCharacters(in: .whitespaces)
            return t.hasPrefix("```") || t.hasPrefix("~~~")
        }
        // odd = the wrapper's close is still there: it's the last fence
        if fences.count % 2 == 1, let last = fences.last,
           ["```", "~~~"].contains(lines[last].trimmingCharacters(in: .whitespaces)),
           lines[(last + 1)...].allSatisfy({ $0.trimmingCharacters(in: .whitespaces).isEmpty }) {
            lines.remove(at: last)
        }
        return lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
