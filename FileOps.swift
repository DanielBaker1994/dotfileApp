import Foundation

private func foBox(_ method: String, _ params: [String: Any],
                   timeout: TimeInterval = 300) -> [String: Any]? {
    guard case .success(let box) = PythonHelper.shared.callSync(method, params, timeout: timeout),
          let dict = box as? [String: Any] else { return nil }
    return dict
}

enum FileOps {
    typealias Change = (from: String?, to: String)

    struct Outcome {
        var changes: [Change] = []
        var failed: String?
        var paths: [String] { changes.map { $0.to } }

        init() {}

        init(json: [String: Any]) {
            changes = (json["changes"] as? [[Any]] ?? []).compactMap { c in
                guard c.count == 2, let to = c[1] as? String else { return nil }
                return (from: c[0] as? String, to: to)
            }
            failed = json["failed"] as? String
        }
    }

    final class UndoStack {
        private(set) var handle = -1
        let limit: Int

        init(limit: Int = 50) {
            self.limit = limit
            handle = foBox("fileops.stack_new", ["limit": limit])?["handle"] as? Int ?? -1
        }

        deinit {
            if handle >= 0 { _ = foBox("fileops.stack_drop", ["handle": handle]) }
        }

        var canUndo: Bool {
            handle >= 0 && (foBox("fileops.stack_state", ["handle": handle])?["canUndo"] as? Bool ?? false)
        }

        var count: Int {
            handle >= 0 ? (foBox("fileops.stack_state", ["handle": handle])?["count"] as? Int ?? 0) : 0
        }

        func forget() {
            if handle >= 0 { _ = foBox("fileops.stack_forget", ["handle": handle]) }
        }

        func collapse(since: Int, _ what: String) {
            if handle >= 0 {
                _ = foBox("fileops.stack_collapse", ["handle": handle, "since": since, "what": what])
            }
        }
    }

    static let shared = UndoStack()

    static var canUndo: Bool { shared.canUndo }

    static func forgetUndo() { shared.forget() }

    static func transfer(_ urls: [URL], into dir: String, move: Bool,
                         undo: UndoStack = shared) -> Outcome {
        Outcome(json: foBox("fileops.transfer", ["paths": urls.map(\.path), "into": dir,
                                                 "move": move, "stack": undo.handle]) ?? [:])
    }

    static func duplicate(_ paths: [String], undo: UndoStack = shared) -> Outcome {
        Outcome(json: foBox("fileops.duplicate", ["paths": paths, "stack": undo.handle]) ?? [:])
    }

    static func trash(_ paths: [String], undo: UndoStack = shared) -> Outcome {
        Outcome(json: foBox("fileops.trash", ["paths": paths, "stack": undo.handle]) ?? [:])
    }

    static func create(_ name: String, in dir: String, folder: Bool,
                       undo: UndoStack = shared) -> Outcome {
        Outcome(json: foBox("fileops.create", ["name": name, "dir": dir,
                                               "folder": folder, "stack": undo.handle]) ?? [:])
    }

    static func recordRename(from: String, to: String, undo: UndoStack = shared) {
        _ = foBox("fileops.record_rename", ["from": from, "to": to, "stack": undo.handle])
    }

    enum Clash: String { case replace, keepBoth, skip }

    static func place(_ items: [(src: String, dst: String)], move: Bool, clash: Clash,
                      undo: UndoStack = shared) -> Outcome {
        Outcome(json: foBox("fileops.place", ["items": items.map { [$0.src, $0.dst] },
                                              "move": move, "clash": clash.rawValue,
                                              "stack": undo.handle]) ?? [:])
    }

    static func undo(_ stack: UndoStack = shared) -> (what: String, outcome: Outcome)? {
        guard let box = foBox("fileops.undo", ["stack": stack.handle]),
              let outcome = box["outcome"] as? [String: Any] else { return nil }
        return (box["what"] as? String ?? "", Outcome(json: outcome))
    }
}
