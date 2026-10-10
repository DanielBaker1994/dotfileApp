import AppKit

final class PathShelf {
    static let shared = PathShelf()
    static let changed = Notification.Name("PathShelfChanged")

    enum Why: String {
        case created, modified, downloaded, clipboard, filefast, copied, screenshot
        var label: String {
            switch self {
            case .created: return "new"
            case .modified: return "edited"
            case .downloaded: return "downloaded"
            case .clipboard: return "copied"
            case .filefast: return "filefast"
            case .copied: return "files view"
            case .screenshot: return "screenshot"
            }
        }
    }

    struct Item: Equatable {
        var path: String
        var at: Double
        var why: Why
    }

    static let maxLimit = 25
    private(set) var limit = maxLimit
    let rules: IgnoreRules
    private let store: String
    private let queue = DispatchQueue(label: "path-shelf")
    private var items: [Item] = []
    private var loaded = false
    private let snapLock = NSLock()
    private var snapshot: [Item] = []
    private var saveWork: DispatchWorkItem?
    private var notifyWork: DispatchWorkItem?
    var immediate = false

    init(store: String = NSHomeDirectory() + "/.cache/kitchen-sink/paths.json",
         rules: IgnoreRules = IgnoreRules()) {
        self.store = store
        self.rules = rules
    }

    func configure(limit: Int, ignoreFile: String?) {
        queue.sync {
            self.limit = min(max(1, limit), Self.maxLimit)
            rules.shelfFile = ignoreFile
            if !loaded { load(); loaded = true }
            items = Array(items.prefix(self.limit))
            publish()
        }
    }

    func entries() -> [Item] {
        snapLock.lock()
        let all = snapshot
        snapLock.unlock()
        return all.filter { FileManager.default.fileExists(atPath: $0.path) }
    }

    var isEmpty: Bool { queue.sync { items.isEmpty } }

    func observe(_ path: String, created: Bool, origin: String?) {
        queue.async { [self] in
            guard loaded, let c = Self.canonical(path), c.isFile, !rules.ignored(c.path) else { return }
            let why: Why = origin != nil ? .downloaded : created ? .created : .modified
            bump(c.path, why)
        }
    }

    func add(_ paths: [String], why: Why) {
        queue.async { [self] in
            guard loaded else { return }
            for p in paths.reversed() {
                guard let c = Self.canonical(Self.normalize(p)), c.isFile || c.isDir else { continue }
                bump(c.path, why)
            }
        }
    }

    func renamed(from old: String, to new: String) {
        queue.async { [self] in
            var hit = false
            for i in items.indices {
                if case .success(let box) = PythonHelper.shared.callSync(
                        "shelf.rekey", ["path": items[i].path, "old": old, "new": new], timeout: 30),
                   let d = box as? [String: Any], let p = d["path"] as? String {
                    items[i].path = p
                    hit = true
                }
            }
            if hit { dedup(); publish() }
        }
    }

    func remove(_ paths: [String]) {
        queue.async { [self] in
            let gone = Set(paths)
            let before = items.count
            items.removeAll { gone.contains($0.path) }
            if items.count != before { publish() }
        }
    }

    func seed(from recent: [(path: String, at: Date, source: String?)]) {
        queue.async { [self] in
            guard loaded, items.isEmpty else { return }
            for e in recent where items.count < limit {
                guard let c = Self.canonical(e.path), c.isFile, !rules.ignored(c.path),
                      !items.contains(where: { $0.path == c.path }) else { continue }
                items.append(Item(path: c.path, at: e.at.timeIntervalSince1970,
                                  why: e.source != nil ? .downloaded : .modified))
            }
            if !items.isEmpty { publish() }
        }
    }

    func sync() { queue.sync {} }

    static func canonical(_ p: String) -> (path: String, isFile: Bool, isDir: Bool)? {
        // full realpath + stat live in pylib/shelf.py
        guard case .success(let box) = PythonHelper.shared.callSync(
                "shelf.canonical", ["path": p], timeout: 30),
              let d = box as? [String: Any], let r = d["result"] as? [String: Any],
              let path = r["path"] as? String else { return nil }
        return (path, r["isFile"] as? Bool ?? false, r["isDir"] as? Bool ?? false)
    }

    static func normalize(_ p: String) -> String {
        if case .success(let box) = PythonHelper.shared.callSync(
                "shelf.normalize", ["path": p], timeout: 30),
           let d = box as? [String: Any], let s = d["path"] as? String {
            return s
        }
        return p
    }

    private func itemsJSON() -> [[String: Any]] {
        items.map { ["path": $0.path, "at": $0.at, "why": $0.why.rawValue] }
    }

    private func setItems(_ arr: [[String: Any]]) {
        items = arr.compactMap { d in
            guard let p = d["path"] as? String, let t = d["at"] as? Double,
                  let why = Why(rawValue: d["why"] as? String ?? "") else { return nil }
            return Item(path: p, at: t, why: why)
        }
    }

    private func bump(_ path: String, _ why: Why) {
        let now = Date().timeIntervalSince1970
        if case .success(let box) = PythonHelper.shared.callSync(
                "shelf.bump", ["items": itemsJSON(), "path": path, "why": why.rawValue,
                               "at": now, "limit": limit], timeout: 30),
           let d = box as? [String: Any], let out = d["items"] as? [[String: Any]] {
            setItems(out)
        }
        publish()
    }

    private func dedup() {
        if case .success(let box) = PythonHelper.shared.callSync(
                "shelf.dedup", ["items": itemsJSON()], timeout: 30),
           let d = box as? [String: Any], let out = d["items"] as? [[String: Any]] {
            setItems(out)
        }
    }

    private func publish() {
        let snap = items
        snapLock.lock()
        snapshot = snap
        snapLock.unlock()
        if immediate {
            NotificationCenter.default.post(name: Self.changed, object: self)
            save()
            return
        }
        notifyWork?.cancel()
        let n = DispatchWorkItem { [weak self] in
            NotificationCenter.default.post(name: Self.changed, object: self)
        }
        notifyWork = n
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: n)
        saveWork?.cancel()
        let s = DispatchWorkItem { [weak self] in self?.save() }
        saveWork = s
        queue.asyncAfter(deadline: .now() + 1.5, execute: s)
    }

    private func load() {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: store)),
              let raw = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        // candidates (canonical + activity rules) come from pylib/shelf.py;
        // the app's ignore rules filter between them and finalize
        var candidates: [[String: Any]] = []
        if case .success(let box) = PythonHelper.shared.callSync("shelf.load", ["raw": raw], timeout: 60),
           let d = box as? [String: Any], let out = d["items"] as? [[String: Any]] {
            candidates = out
        }
        candidates = candidates.filter {
            guard let p = $0["path"] as? String else { return false }
            let why = $0["why"] as? String ?? ""
            // the ignore rules only gate activity entries; a clipboard /
            // filefast / screenshot path skips them (same as the old load)
            if ["created", "modified", "downloaded"].contains(why) { return !rules.ignored(p) }
            return true
        }
        if case .success(let box) = PythonHelper.shared.callSync(
                "shelf.finalize", ["items": candidates, "limit": limit], timeout: 60),
           let d = box as? [String: Any], let out = d["items"] as? [[String: Any]] {
            setItems(out)
        }
    }

    private func save() {
        _ = PythonHelper.shared.callSync("shelf.save", ["path": store, "items": itemsJSON()], timeout: 30)
    }
}

final class IgnoreRules {
    private var handle = -1

    var shelfFile: String? {
        didSet {
            guard handle >= 0, shelfFile != oldValue else { return }
            _ = irBox("ignore.set_shelf", ["handle": handle, "file": shelfFile ?? NSNull()])
        }
    }

    var gitExcludes: String? {
        didSet {
            guard handle >= 0, gitExcludes != oldValue else { return }
            _ = irBox("ignore.set_git_excludes", ["handle": handle, "path": gitExcludes ?? NSNull()])
        }
    }

    var recheck: Double = 2 {
        didSet { if handle >= 0 { _ = irBox("ignore.set_recheck", ["handle": handle, "seconds": recheck]) } }
    }

    init(home: String = NSHomeDirectory(), shelfFile: String? = nil) {
        if let box = irBox("ignore.new", ["home": home, "shelfFile": shelfFile ?? NSNull()]) {
            handle = box["handle"] as? Int ?? -1
        }
    }

    deinit {
        if handle >= 0 { _ = irBox("ignore.drop", ["handle": handle]) }
    }

    func ignored(_ path: String, isDir: Bool = false) -> Bool {
        guard handle >= 0,
              case .success(let box) = PythonHelper.shared.callSync(
                "ignore.ignored", ["handle": handle, "path": path, "isDir": isDir], timeout: 30),
              let dict = box as? [String: Any] else { return false }
        return dict["ignored"] as? Bool ?? false
    }
}

private func irBox(_ method: String, _ params: [String: Any],
                   timeout: TimeInterval = 60) -> [String: Any]? {
    guard case .success(let box) = PythonHelper.shared.callSync(method, params, timeout: timeout),
          let dict = box as? [String: Any] else { return nil }
    return dict
}

final class ClipboardPaths {
    let pasteboard: NSPasteboard
    var onPaths: (([String]) -> Void)?
    private var lastCount: Int
    private var ownCount: Int?
    private var timer: Timer?

    static let skipTypes: Set<String> = [
        "org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType",
        "org.nspasteboard.AutoGeneratedType", "com.agilebits.onepassword",
    ]

    init(pasteboard: NSPasteboard = .general) {
        self.pasteboard = pasteboard
        lastCount = pasteboard.changeCount
    }

    func start(interval: Double = 0.5) {
        guard timer == nil else { return }
        lastCount = pasteboard.changeCount
        let t = Timer(timeInterval: interval, repeats: true) { [weak self] _ in self?.check() }
        t.tolerance = interval / 2
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    func ownWrite() { ownCount = pasteboard.changeCount }

    func check() {
        let n = pasteboard.changeCount
        guard n != lastCount else { return }
        lastCount = n
        if n == ownCount { return }
        let paths = Self.paths(in: pasteboard)
        if !paths.isEmpty { onPaths?(paths) }
    }

    static func paths(in pb: NSPasteboard) -> [String] {
        let types = Set((pb.types ?? []).map(\.rawValue))
        guard types.isDisjoint(with: skipTypes) else { return [] }
        if types.contains(NSPasteboard.PasteboardType.fileURL.rawValue),
           let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
           !urls.isEmpty {
            return urls.prefix(25).compactMap { PathShelf.canonical($0.path)?.path }
        }
        guard let text = pb.string(forType: .string) else { return [] }
        return paths(inText: text)
    }

    static func paths(inText text: String) -> [String] {
        guard text.count <= 4096 else { return [] }
        let lines = text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        guard (1...5).contains(lines.count) else { return [] }
        var out: [String] = []
        for var l in lines {
            if l.count > 1, let f = l.first, f == l.last, f == "'" || f == "\"" { l = String(l.dropFirst().dropLast()) }
            if l.hasPrefix("file://") {
                guard let u = URL(string: l), u.isFileURL else { return [] }
                l = u.path
            } else {
                l = l.replacingOccurrences(of: "\\ ", with: " ")
            }
            l = (l as NSString).expandingTildeInPath
            guard l.hasPrefix("/"), let c = PathShelf.canonical(l) else { return [] }
            out.append(c.path)
        }
        return out
    }
}
