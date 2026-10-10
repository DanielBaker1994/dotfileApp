import Foundation

struct PaneShotArgs: Equatable {
    var pane: String?
    var lines: Int?
    var all = false
    var file: String?
    var save: Bool?
    var copy: Bool?

    struct Problem: Error, Equatable { let message: String }

    /// The CLI grammar lives in pylib/paneshot.py (its suite holds the old
    /// Swift cases); this stays the typed mirror.
    static func parse(_ words: [String]) -> Result<PaneShotArgs, Problem> {
        guard case .success(let box) = PythonHelper.shared.callSync(
                "paneshot.args", ["words": words], timeout: 30),
              let d = box as? [String: Any] else {
            return .failure(Problem(message: "python helper unavailable"))
        }
        guard d["ok"] as? Bool == true else {
            return .failure(Problem(message: d["message"] as? String ?? "bad arguments"))
        }
        let a = d["args"] as? [String: Any] ?? [:]
        var out = PaneShotArgs()
        out.pane = a["pane"] as? String
        out.lines = a["lines"] as? Int
        out.all = a["all"] as? Bool ?? false
        out.file = a["file"] as? String
        out.save = a["save"] as? Bool
        out.copy = a["copy"] as? Bool
        return .success(out)
    }
}

struct PaneShotConfig {
    var lines = 200
    var save = true
    var copy = true
    var preview = true
    var herdrBin = "~/.local/bin/herdr"
    var ghosttyBin = "/Applications/Ghostty.app/Contents/MacOS/ghostty"
    var font = ""
    var fontSize: Double = 0
    var background = ""
    var padding: Double = 16
    var savePath = ""
    var filenamePattern = "%F_%H-%M-%S pane"
    var toast = "Screenshot of {pane} copied ({n} lines)"

    init() {}

    /// entries -> clamps live in pylib/paneshot.py; defaults below are the
    /// same table, used when the worker cannot answer.
    init(_ e: [String: String]) {
        var d: [String: Any] = [:]
        if case .success(let box) = PythonHelper.shared.callSync(
                "paneshot.config", ["entries": e], timeout: 30),
           let c = box as? [String: Any], let cfg = c["config"] as? [String: Any] {
            d = cfg
        }
        lines = d["lines"] as? Int ?? 200
        save = d["save"] as? Bool ?? true
        copy = d["copy"] as? Bool ?? true
        preview = d["preview"] as? Bool ?? true
        herdrBin = d["herdrBin"] as? String ?? herdrBin
        ghosttyBin = d["ghosttyBin"] as? String ?? ghosttyBin
        font = d["font"] as? String ?? ""
        fontSize = d["fontSize"] as? Double ?? 0
        background = d["background"] as? String ?? ""
        padding = d["padding"] as? Double ?? 16
        savePath = d["savePath"] as? String ?? ""
        filenamePattern = d["filenamePattern"] as? String ?? filenamePattern
        toast = d["toast"] as? String ?? toast
    }
}

enum Herdr {
    static let maxLines = 1000

    struct Pane: Equatable {
        var id: String
        var title: String
        var viewportRows: Int
    }

    struct Failure: Error, Equatable { let message: String }

    static func pane(fromJSON text: String) -> Result<Pane, Failure> {
        // the envelope lives in pylib/paneshot.py
        guard case .success(let box) = PythonHelper.shared.callSync(
                "paneshot.pane_json", ["text": text], timeout: 30),
              let d = box as? [String: Any] else {
            return .failure(Failure(message: "herdr answered no JSON"))
        }
        guard d["ok"] as? Bool == true else {
            return .failure(Failure(message: d["message"] as? String ?? "herdr error"))
        }
        let p = d["pane"] as? [String: Any] ?? [:]
        return .success(Pane(id: p["id"] as? String ?? "",
                             title: p["title"] as? String ?? "",
                             viewportRows: p["viewportRows"] as? Int ?? 0))
    }

    static func lines(viewport: Int, history: Int, all: Bool) -> Int {
        if case .success(let box) = PythonHelper.shared.callSync(
                "paneshot.lines", ["viewport": viewport, "history": history, "all": all], timeout: 30),
           let d = box as? [String: Any], let n = d["lines"] as? Int {
            return n
        }
        return all ? maxLines : min(maxLines, max(1, viewport + history))
    }

    static func environment(_ env: [String: String] = ProcessInfo.processInfo.environment) -> [String: String] {
        if case .success(let box) = PythonHelper.shared.callSync(
                "paneshot.environment", ["env": env], timeout: 30),
           let d = box as? [String: Any], let e = d["env"] as? [String: String] {
            return e
        }
        return env.filter { !["HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"].contains($0.key) }
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
            let msg = r.err.trimmingCharacters(in: .whitespacesAndNewlines)
            if msg.hasPrefix("{"), case .failure(let f) = pane(fromJSON: msg) { return .failure(f) }
            return .failure(Failure(message: msg.isEmpty ? "herdr exited \(r.code)" : msg))
        }
        return .success(r.out)
    }

    static func pane(_ bin: String, id: String?) -> Result<Pane, Failure> {
        run(bin, id.map { ["pane", "get", $0] } ?? ["pane", "current"]).flatMap { pane(fromJSON: $0) }
    }

    static func read(_ bin: String, id: String, lines: Int) -> Result<String, Failure> {
        run(bin, ["pane", "read", id, "--source", "recent", "--format", "ansi", "--lines", String(lines)])
    }
}
