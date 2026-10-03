// PaneShot.swift — /pane-shot: a full-height image of the focused herdr
// pane (its scrollback + the visible screen), copied + saved like a
// /screenshot capture. Why herdr and not Ghostty: herdr draws its whole UI
// as one full-screen app inside Ghostty, so Ghostty's own scrollback never
// holds a single pane's history — herdr's server does, and `herdr pane read
// --format ansi` hands it over with its colors (≤ 1000 rows, a server cap).
//   this file          CLI args, [pane-shot] config, the herdr calls
//   AnsiRender.swift   ANSI → cell grid → image (Ghostty's font + palette)
//   Screenshot.swift   `ScreenshotController.paneShot` (outputs, toast)
// Foundation only: bin/run-tests.sh ansi compiles it with AnsiRender.swift.

import Foundation

// `workspace-switcher pane-shot [--pane ID] [--lines N|all] [--file PATH|-]
// [--no-save] [--no-copy]`
struct PaneShotArgs: Equatable {
    var pane: String?
    var lines: Int?          // history rows above the screen; nil = [pane-shot] lines
    var all = false          // every row herdr hands out (Herdr.maxLines)
    var file: String?        // render this ANSI file instead of a pane
    var save: Bool?          // nil = [pane-shot] save
    var copy: Bool?          // nil = [pane-shot] copy

    struct Problem: Error, Equatable { let message: String }

    static let usage = "pane-shot [--pane ID] [--lines N|all] [--file PATH|-] [--no-save] [--no-copy]"

    static func parse(_ words: [String]) -> Result<PaneShotArgs, Problem> {
        var a = PaneShotArgs()
        var i = 0
        func value(_ flag: String) -> Result<String, Problem> {
            i += 1
            guard i < words.count, !words[i].isEmpty else { return .failure(Problem(message: "\(flag) needs a value")) }
            return .success(words[i])
        }
        while i < words.count {
            let w = words[i]
            switch w {
            case "--pane", "-p":
                switch value(w) { case .success(let v): a.pane = v; case .failure(let p): return .failure(p) }
            case "--lines", "-n":
                switch value(w) {
                case .success(let v):
                    if v == "all" { a.all = true }
                    else if let n = Int(v), n >= 0 { a.lines = n }
                    else { return .failure(Problem(message: "--lines takes a number or \"all\", not \(v)")) }
                case .failure(let p): return .failure(p)
                }
            case "--file", "-f":
                switch value(w) { case .success(let v): a.file = v; case .failure(let p): return .failure(p) }
            case "--save": a.save = true
            case "--no-save": a.save = false
            case "--copy": a.copy = true
            case "--no-copy": a.copy = false
            case "-h", "--help": return .failure(Problem(message: "usage: workspace-switcher " + usage))
            default: return .failure(Problem(message: "unknown argument \(w)\nusage: workspace-switcher " + usage))
            }
            i += 1
        }
        return .success(a)
    }
}

// [pane-shot] in commands.toml (not a palette command)
struct PaneShotConfig {
    var lines = 200
    var save = true
    var copy = true
    var preview = true           // open the capture in the preview popup
    var herdrBin = "~/.local/bin/herdr"
    var ghosttyBin = "/Applications/Ghostty.app/Contents/MacOS/ghostty"
    var font = ""                // "" = Ghostty's font-family
    var fontSize: Double = 0     // 0 = Ghostty's font-size
    var background = ""          // "" = Ghostty's background
    var padding: Double = 16
    var savePath = ""            // "" = [screenshot] save-path
    var filenamePattern = "%F_%H-%M-%S pane"
    var toast = "Screenshot of {pane} copied ({n} lines)"

    init() {}

    init(_ e: [String: String]) {
        func s(_ k: String, _ d: String) -> String {
            guard let v = e[k]?.trimmingCharacters(in: .whitespaces), !v.isEmpty else { return d }
            return v
        }
        func n(_ k: String, _ d: Double, _ r: ClosedRange<Double>) -> Double {
            guard let v = e[k].flatMap({ Double($0) }) else { return d }
            return max(r.lowerBound, min(r.upperBound, v))
        }
        lines = Int(n("lines", 200, 0...Double(Herdr.maxLines)))
        save = tri(e["save"]) ?? true
        copy = tri(e["copy"]) ?? true
        preview = tri(e["preview"]) ?? true
        herdrBin = s("herdr-bin", herdrBin)
        ghosttyBin = s("ghostty-bin", ghosttyBin)
        font = s("font", "")
        fontSize = n("font-size", 0, 0...72)
        background = s("background", "")
        padding = n("padding", 16, 0...200)
        savePath = s("save-path", "")
        filenamePattern = s("filename-pattern", filenamePattern)
        toast = e["toast"] ?? toast
    }
}

// the herdr CLI (JSON answers on stdout, `{"error":{…}}` + exit 1 on failure)
enum Herdr {
    static let maxLines = 1000   // herdr's server-side cap on pane.read

    struct Pane: Equatable {
        var id: String
        var title: String
        var viewportRows: Int
    }

    struct Failure: Error, Equatable { let message: String }

    // `pane current` / `pane get` output → the pane
    static func pane(fromJSON text: String) -> Result<Pane, Failure> {
        guard let d = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] else {
            return .failure(Failure(message: "herdr answered no JSON"))
        }
        if let e = d["error"] as? [String: Any] {
            return .failure(Failure(message: e["message"] as? String ?? "herdr error"))
        }
        guard let p = (d["result"] as? [String: Any])?["pane"] as? [String: Any], let id = p["pane_id"] as? String else {
            return .failure(Failure(message: "herdr answered without a pane"))
        }
        let rows = (p["scroll"] as? [String: Any])?["viewport_rows"] as? Int ?? 0
        let title = (p["terminal_title_stripped"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            ?? (p["agent"] as? String) ?? id
        return .success(Pane(id: id, title: title, viewportRows: rows))
    }

    // rows to ask for: the screen + `history`, within herdr's cap
    static func lines(viewport: Int, history: Int, all: Bool) -> Int {
        all ? maxLines : min(maxLines, max(1, viewport + history))
    }

    // herdr's env vars name the pane THIS process runs in (a daemon started
    // from a pane inherits them): drop them so `pane current` = the focused
    // pane. HERDR_SOCKET_PATH stays (it picks the session).
    static func environment(_ env: [String: String] = ProcessInfo.processInfo.environment) -> [String: String] {
        env.filter { !["HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"].contains($0.key) }
    }

    static func run(_ bin: String, _ args: [String]) -> Result<String, Failure> {
        let exe = (bin as NSString).expandingTildeInPath
        guard FileManager.default.isExecutableFile(atPath: exe) else {
            return .failure(Failure(message: "herdr not found at \(bin) ([pane-shot] herdr-bin)"))
        }
        guard let r = try? runProcess(exe, args, env: environment()) else {
            return .failure(Failure(message: "could not run \(bin)"))
        }
        if r.code != 0 {
            // errors come as `{"error":{…}}` on stderr
            let msg = r.err.trimmingCharacters(in: .whitespacesAndNewlines)
            if msg.hasPrefix("{"), case .failure(let f) = pane(fromJSON: msg) { return .failure(f) }
            return .failure(Failure(message: msg.isEmpty ? "herdr exited \(r.code)" : msg))
        }
        return .success(r.out)
    }

    // the pane to shoot: `id`, else herdr's focused pane
    static func pane(_ bin: String, id: String?) -> Result<Pane, Failure> {
        run(bin, id.map { ["pane", "get", $0] } ?? ["pane", "current"]).flatMap { pane(fromJSON: $0) }
    }

    static func read(_ bin: String, id: String, lines: Int) -> Result<String, Failure> {
        run(bin, ["pane", "read", id, "--source", "recent", "--format", "ansi", "--lines", String(lines)])
    }
}
