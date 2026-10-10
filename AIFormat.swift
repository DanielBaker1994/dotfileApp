import AppKit

enum PasteTarget: Int {
    case outlook = 0, webex = 1
    var title: String { self == .outlook ? "Outlook" : "Webex" }
    var key: String { self == .outlook ? "outlook" : "webex" }
}

enum PaneMode: String {
    case diff, markdown, outlook, webex
    var title: String { rawValue.prefix(1).uppercased() + rawValue.dropFirst() }
    var target: PasteTarget? { self == .outlook ? .outlook : self == .webex ? .webex : nil }
    static func all(diff: Bool) -> [PaneMode] { (diff ? [.diff] : []) + [.markdown, .outlook, .webex] }
}

private func aiBox(_ method: String, _ params: [String: Any],
                   timeout: TimeInterval = 60) -> [String: Any]? {
    guard case .success(let box) = PythonHelper.shared.callSync(method, params, timeout: timeout),
          let dict = box as? [String: Any] else { return nil }
    return dict
}

private func aiString(_ method: String, _ params: [String: Any],
                      _ key: String, timeout: TimeInterval = 60) -> String? {
    aiBox(method, params, timeout: timeout)?[key] as? String
}

enum RichText {
    static var pandocBin: String { (aiSetting("pandoc-bin", "/opt/homebrew/bin/pandoc") as NSString).expandingTildeInPath }
    static var available: Bool { FileManager.default.isExecutableFile(atPath: pandocBin) }

    static func tablesAsText(_ md: String) -> String {
        aiString("ai.tables_as_text", ["md": md], "text") ?? md
    }

    static func markdown(_ md: String, for t: PasteTarget) -> String {
        aiString("ai.markdown", ["md": md, "target": t.key], "text") ?? md
    }

    static func pandocHTML(_ md: String, highlight: Bool = false) -> String? {
        guard available else { return nil }
        return aiString("ai.pandoc_html",
                        ["md": md, "highlight": highlight, "pandoc": pandocBin], "html")
    }

    static func styled(_ fragment: String, for t: PasteTarget) -> String {
        aiString("ai.styled", ["fragment": fragment, "target": t.key], "html") ?? fragment
    }

    static func html(_ md: String, for t: PasteTarget) -> String? {
        guard available else { return nil }
        return aiString("ai.html", ["md": md, "target": t.key, "pandoc": pandocBin], "html")
    }

    static func document(_ fragment: String) -> String {
        "<html><head><meta charset=\"utf-8\"></head><body>\(fragment)</body></html>"
    }

    static func rtf(_ fragment: String) -> Data? {
        guard let a = try? NSAttributedString(data: Data(document(fragment).utf8),
                                              options: [.documentType: NSAttributedString.DocumentType.html,
                                                        .characterEncoding: String.Encoding.utf8.rawValue],
                                              documentAttributes: nil) else { return nil }
        return try? a.data(from: NSRange(location: 0, length: a.length),
                           documentAttributes: [.documentType: NSAttributedString.DocumentType.rtf])
    }

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

struct CodeGuard {
    var text: String
    var codes: [String] = []

    static func token(_ n: Int) -> String { "[[CODE\(n)]]" }

    init(_ s: String) {
        if let box = aiBox("ai.code_guard", ["text": s]) {
            text = box["text"] as? String ?? s
            codes = box["codes"] as? [String] ?? []
        } else {
            text = s
        }
    }

    func restore(_ s: String) -> (text: String, missing: Int) {
        guard let box = aiBox("ai.code_restore", ["codes": codes, "s": s]) else { return (s, 0) }
        return (box["text"] as? String ?? s, box["missing"] as? Int ?? 0)
    }
}

enum TokenBudget {
    static var context: Int { max(512, Int(aiNumber("context-tokens", 4096))) }

    static func estimate(_ s: String) -> Int {
        if let n = aiBox("ai.estimate", ["s": s])?["tokens"] as? Int { return n }
        return Int((Double(s.utf8.count) / 3.2).rounded(.up))
    }

    static func partBudget(instructions: String) -> Int {
        aiBox("ai.part_budget", ["instructions": instructions, "context": context])?["budget"] as? Int
            ?? 200
    }

    static func parts(_ s: String, budget: Int) -> [String] {
        aiBox("ai.parts", ["s": s, "budget": budget])?["parts"] as? [String] ?? [s]
    }
}

enum Reflow {
    static func instructions(_ s: String) -> String {
        aiString("ai.reflow", ["s": s], "text") ?? s
    }
}

enum AnswerCleanup {
    static func unwrapFence(_ s: String) -> String {
        aiString("ai.unwrap_fence", ["s": s], "text") ?? s
    }
}

struct AIRule {
    let path: String
    var name: String
    var output = "plain"
    var placeholder = ""
    var flags: [String] = []
    var instructions = ""
    var warnings: [String] = []
    var protectCodeSet: Bool?
    var chunkSet: Bool?
    var prompt = ""
    var then = ""
    var keepWords = false
    var csvTables = false

    var file: String { (path as NSString).lastPathComponent }
    var diff: Bool { output == "diff" }
    var protectCode: Bool { protectCodeSet ?? true }
    var chunk: Bool { chunkSet ?? diff }

    var json: [String: Any] {
        ["path": path, "name": name, "output": output, "placeholder": placeholder,
         "flags": flags, "instructions": instructions, "warnings": warnings,
         "protectCodeSet": protectCodeSet ?? NSNull(), "chunkSet": chunkSet ?? NSNull(),
         "prompt": prompt, "then": then, "keepWords": keepWords, "csvTables": csvTables]
    }

    init(path: String, name: String) {
        self.path = path
        self.name = name
    }

    init(json: [String: Any]) {
        path = json["path"] as? String ?? ""
        name = json["name"] as? String ?? ""
        output = json["output"] as? String ?? "plain"
        placeholder = json["placeholder"] as? String ?? ""
        flags = json["flags"] as? [String] ?? []
        instructions = json["instructions"] as? String ?? ""
        warnings = json["warnings"] as? [String] ?? []
        protectCodeSet = json["protectCodeSet"] as? Bool
        chunkSet = json["chunkSet"] as? Bool
        prompt = json["prompt"] as? String ?? ""
        then = json["then"] as? String ?? ""
        keepWords = json["keepWords"] as? Bool ?? false
        csvTables = json["csvTables"] as? Bool ?? false
    }

    static func load(_ path: String) -> AIRule {
        guard let j = aiBox("ai.rule_load", ["path": path])?["rule"] as? [String: Any] else {
            return AIRule(path: path, name: ((path as NSString).lastPathComponent as NSString).deletingPathExtension)
        }
        return AIRule(json: j)
    }

    static func chain(_ first: AIRule) -> [AIRule] {
        guard let list = aiBox("ai.rule_chain", ["path": first.path])?["rules"] as? [[String: Any]] else {
            return [first]
        }
        return list.map(AIRule.init(json:))
    }

    func instructions(guarded: Bool) -> String {
        aiString("ai.rule_instructions", ["rule": json, "guarded": guarded], "text") ?? instructions
    }

    func wrap(_ text: String) -> String {
        aiString("ai.rule_wrap", ["rule": json, "text": text], "text") ?? text
    }

    func prepare(_ text: String) -> String {
        aiString("ai.rule_prepare", ["rule": json, "text": text], "text") ?? text
    }

    func accept(input: String, answer: String) -> (text: String, note: String?) {
        guard let box = aiBox("ai.rule_accept", ["rule": json, "input": input, "answer": answer]) else {
            return (answer, nil)
        }
        return (box["text"] as? String ?? answer, box["note"] as? String)
    }

    func arguments(guarded: Bool) -> [String] {
        aiBox("ai.rule_arguments", ["rule": json, "guarded": guarded])?["args"] as? [String] ?? []
    }

    var preview: String {
        aiString("ai.rule_preview", ["rule": json], "text") ?? ""
    }

    func runnable(input: String) -> String {
        aiString("ai.rule_runnable", ["rule": json, "input": input], "text") ?? ""
    }
}

enum CSVTables {
    static func cells(_ line: String) -> [String]? {
        aiBox("ai.csv_cells", ["line": line])?["cells"] as? [String]
    }

    static func convert(_ s: String) -> String {
        aiString("ai.csv_convert", ["s": s], "text") ?? s
    }
}

enum WordGuard {
    static func words(_ s: String) -> [String] {
        aiBox("ai.words", ["s": s])?["words"] as? [String] ?? []
    }

    static func check(_ input: String, _ output: String) -> (ok: Bool, added: [String], dropped: [String]) {
        guard let box = aiBox("ai.word_check", ["input": input, "output": output]) else {
            return (true, [], [])
        }
        return (box["ok"] as? Bool ?? true,
                box["added"] as? [String] ?? [],
                box["dropped"] as? [String] ?? [])
    }
}
