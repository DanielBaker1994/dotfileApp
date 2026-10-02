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
        guard available,
              let r = try? runProcess(pandocBin, ["-f", "gfm", "-t", "html", "--syntax-highlighting=none",
                                                  "--wrap=none"], stdin: md),
              r.code == 0 else { return nil }
        return r.out
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

// MARK: - rule files

struct AIRule {
    let path: String
    var name: String
    var output = "plain"            // diff | plain
    var placeholder = ""
    var flags: [String] = []        // fm respond options, in file order
    var instructions = ""
    var warnings: [String] = []
    var protectCodeSet: Bool?       // protect-code (default on)
    var chunkSet: Bool?             // chunk: split long text (default: diff rules)
    var prompt = ""                 // a line put before the text ("Proofread this draft:")
    var then = ""                   // the rule file that gets this rule's answer next
    var keepWords = false           // an answer that changes the words is dropped
    var csvTables = false           // comma rows become a Markdown table first

    var file: String { (path as NSString).lastPathComponent }
    var diff: Bool { output == "diff" }
    var protectCode: Bool { protectCodeSet ?? true }
    var chunk: Bool { chunkSet ?? diff }

    // `---` frontmatter (key: value, # comments) + the body as instructions
    static func load(_ path: String) -> AIRule {
        let text = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
        let base = ((path as NSString).lastPathComponent as NSString).deletingPathExtension
        var r = AIRule(path: path, name: base)
        var lines = text.components(separatedBy: "\n")
        if lines.first?.trimmingCharacters(in: .whitespaces) == "---",
           let end = lines.dropFirst().firstIndex(where: { $0.trimmingCharacters(in: .whitespaces) == "---" }) {
            for raw in lines[1..<end] {
                var line = raw
                if let h = line.range(of: #"(^|\s)#.*$"#, options: .regularExpression) { line.removeSubrange(h) }
                guard let colon = line.firstIndex(of: ":") else { continue }
                let key = line[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
                var val = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                if val.count >= 2, let f = val.first, f == "\"" || f == "'", val.last == f {
                    val = String(val.dropFirst().dropLast())
                }
                guard !key.isEmpty, !val.isEmpty else { continue }
                let on = ["true", "yes", "1", "on"].contains(val.lowercased())
                switch key {
                case "name": r.name = val
                case "output": r.output = val.lowercased()
                case "placeholder": r.placeholder = val
                case "greedy": if on { r.flags.append("--greedy") }
                case "guardrails": r.flags += ["--guardrails", val]
                case "use-case", "use_case", "usecase": r.flags += ["--use-case", val]
                case "model": r.flags += ["-m", val]
                case "protect-code": r.protectCodeSet = on
                case "chunk": r.chunkSet = on
                case "prompt": r.prompt = val
                case "then": r.then = val.lowercased().hasSuffix(".md") ? val : val + ".md"
                case "keep-words": r.keepWords = on
                case "csv-tables": r.csvTables = on
                default: r.warnings.append("unknown key “\(key)”")
                }
            }
            lines = Array(lines[(end + 1)...])
        }
        r.instructions = lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        return r
    }

    // this rule, then every rule its `then:` leads to (same folder; a
    // missing file or a loop ends the chain with a warning on the first)
    static func chain(_ first: AIRule) -> [AIRule] {
        var out = [first]
        let dir = (first.path as NSString).deletingLastPathComponent
        while let next = out.last?.then, !next.isEmpty, out.count < 6 {
            let path = dir + "/" + next
            if out.contains(where: { $0.file == next }) {
                out[0].warnings.append("then: “\(next)” loops")
                break
            }
            guard FileManager.default.fileExists(atPath: path) else {
                out[0].warnings.append("then: no “\(next)”")
                break
            }
            out.append(load(path))
        }
        return out
    }

    // the instructions a run sends (plus the code-token rule when needed)
    func instructions(guarded: Bool) -> String {
        let i = Reflow.instructions(instructions)
        return guarded ? i + "\n- " + CodeGuard.instruction : i
    }

    // what goes to fm on stdin: `prompt:` on its own line, then the text
    func wrap(_ text: String) -> String { prompt.isEmpty ? text : prompt + "\n\n" + text }

    // what the rule does to its text before the model sees it
    func prepare(_ text: String) -> String { csvTables ? CSVTables.convert(text) : text }

    // the model's answer to `input` (what prepare returned), or the input
    // back when a keep-words rule's answer changed the words
    func accept(input: String, answer: String) -> (text: String, note: String?) {
        guard keepWords else { return (answer, nil) }
        let c = WordGuard.check(input, answer)
        guard !c.ok else { return (answer, nil) }
        let what = [c.added.isEmpty ? nil : "added “\(c.added.prefix(3).joined(separator: " "))”",
                    c.dropped.isEmpty ? nil : "dropped “\(c.dropped.prefix(3).joined(separator: " "))”"]
            .compactMap { $0 }.joined(separator: ", ")
        return (input, "“\(name)” skipped: it \(what)")
    }

    // the argv after the fm binary; the input goes on stdin
    func arguments(guarded: Bool) -> [String] {
        let i = instructions(guarded: guarded)
        return ["respond", "--stream"] + (i.isEmpty ? [] : ["-i", i]) + flags
    }

    // what the view shows: the instructions by file, the input by name
    var preview: String {
        (["fm", "respond"] + (instructions.isEmpty ? [] : ["-i", "@rules/" + file]) + flags.map(shq))
            .joined(separator: " ") + " < input" + (then.isEmpty ? "" : "  → then " + then)
    }

    // a command you can paste into a shell (instructions + input inline)
    func runnable(input: String) -> String {
        let args = ["fm", "respond"] + (instructions.isEmpty ? [] : ["-i", shq(Reflow.instructions(instructions))]) + flags.map(shq)
        return args.joined(separator: " ") + " <<'WS_INPUT'\n" + wrap(prepare(input)) + "\nWS_INPUT"
    }
}

private func shq(_ s: String) -> String {
    s.range(of: #"^[A-Za-z0-9_@./:=+-]+$"#, options: .regularExpression) != nil
        ? s : "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

// comma rows typed as a quick table ("name,status" / "---" / "web01,up")
// -> a Markdown table. Done here, not by the model: it's exact every time.
enum CSVTables {
    // a row's cells, or nil when the line doesn't look like one
    static func cells(_ line: String) -> [String]? {
        let t = line.trimmingCharacters(in: .whitespaces)
        guard t.contains(","), !["|", "#", "- ", "* ", "> "].contains(where: { t.hasPrefix($0) }) else { return nil }
        let c = t.components(separatedBy: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard c.count >= 2, c.allSatisfy({ cell in
            !cell.isEmpty && cell.count <= 40 && cell.split(separator: " ").count <= 5
                && !(cell.count > 1 && [".", "?", "!"].contains(where: { cell.hasSuffix($0) }))
        }) else { return nil }
        return c
    }

    private static func isRule(_ line: String) -> Bool {
        line.trimmingCharacters(in: .whitespaces).range(of: #"^[-=]{3,}$"#, options: .regularExpression) != nil
    }

    static func convert(_ s: String) -> String {
        let lines = s.components(separatedBy: "\n")
        var out: [String] = []
        var i = 0
        var inFence = false
        while i < lines.count {
            let t = lines[i].trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") { inFence.toggle() }
            if !inFence, let head = cells(lines[i]) {
                var j = i + 1
                let ruled = j < lines.count && isRule(lines[j])
                if ruled { j += 1 }
                var rows: [[String]] = []
                while j < lines.count, let r = cells(lines[j]), r.count == head.count {
                    rows.append(r)
                    j += 1
                }
                // without the --- line it takes more to be sure it's a table
                if ruled ? rows.count >= 1 : (rows.count >= 2 || (rows.count == 1 && head.count >= 3)) {
                    func row(_ c: [String]) -> String {
                        "| " + c.map { $0.replacingOccurrences(of: "|", with: "\\|") }.joined(separator: " | ") + " |"
                    }
                    if let last = out.last, !last.trimmingCharacters(in: .whitespaces).isEmpty { out.append("") }
                    out.append(row(head))
                    out.append(row(head.map { _ in "---" }))
                    out += rows.map(row)
                    if j < lines.count, !lines[j].trimmingCharacters(in: .whitespaces).isEmpty { out.append("") }
                    i = j
                    continue
                }
            }
            out.append(lines[i])
            i += 1
        }
        return out.joined(separator: "\n")
    }
}

// a layout-only rule (keep-words) may move the words, never change them:
// nothing new (a table's header row aside), nothing lost but filler
enum WordGuard {
    static let filler: Set<String> = [
        "first", "second", "third", "fourth", "fifth", "firstly", "secondly", "thirdly", "then", "next",
        "finally", "lastly", "and", "also", "is", "are", "was", "were", "has", "have", "had", "with", "the",
        "a", "an", "of", "at", "in", "on", "it", "to", "for", "that", "which", "or",
    ]

    // lowercase words, table header rows and list numbers left out
    static func words(_ s: String) -> [String] {
        let lines = s.components(separatedBy: "\n")
        var kept: [String] = []
        for (i, l) in lines.enumerated() {
            let sep = l.range(of: #"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$"#, options: .regularExpression) != nil
            let nextSep = i + 1 < lines.count && lines[i + 1].contains("|")
                && lines[i + 1].range(of: #"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$"#, options: .regularExpression) != nil
            if (sep && l.contains("|")) || (nextSep && l.contains("|")) { continue }
            // "1." of a numbered list is a marker, not a word
            kept.append(l.replacingOccurrences(of: #"^\s*\d+[.)]\s"#, with: "", options: .regularExpression))
        }
        return kept.joined(separator: "\n").lowercased()
            .components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty }
    }

    static func check(_ input: String, _ output: String) -> (ok: Bool, added: [String], dropped: [String]) {
        var count: [String: Int] = [:]
        for w in words(input) { count[w, default: 0] += 1 }
        var added: [String] = []
        for w in words(output) {
            if let n = count[w], n > 0 { count[w] = n - 1 } else if !filler.contains(w) { added.append(w) }
        }
        let dropped = words(input).filter { w in
            guard let n = count[w], n > 0, !filler.contains(w) else { return false }
            count[w] = n - 1
            return true
        }
        return (added.isEmpty && dropped.isEmpty, added, dropped)
    }
}
