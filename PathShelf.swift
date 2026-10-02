import AppKit

// MARK: - Path shelf (the /paths popup's data)
//
// The ≤25 files you most recently created, changed, downloaded or copied,
// newest first — for sharing them (PathsWindow.swift). Inclusive intake,
// discerning rejection, the way git / ripgrep decide what is junk:
//   activity   the ONE FSEvents stream RecentFiles already runs (its own
//              noise filters first: system trees, ~/Library, hidden paths,
//              node_modules, swap files, half-done downloads), files only,
//              then IgnoreRules — your repo's .gitignore / .ignore /
//              .rgignore, the global git excludes and config/paths.ignore
//   clipboard  a file copied in Finder, or 1-5 lines of text that are each
//              an existing path (ClipboardPaths; password managers skipped)
//   explicit   filefast saves, the Files view's Copy Path / drag-out
// Clipboard + explicit paths skip the ignore rules (you asked for that
// file). Stored in ~/.cache/workspace-switcher/paths.json.
final class PathShelf {
    static let shared = PathShelf()
    static let changed = Notification.Name("PathShelfChanged")

    enum Why: String {
        case created, modified, downloaded, clipboard, filefast, copied
        // the trailing word on the popup's row
        var label: String {
            switch self {
            case .created: return "new"
            case .modified: return "edited"
            case .downloaded: return "downloaded"
            case .clipboard: return "copied"
            case .filefast: return "filefast"
            case .copied: return "files view"
            }
        }
    }

    struct Item: Equatable {
        var path: String
        var at: Double          // epoch s
        var why: Why
    }

    // a hard cap: the shelf is for "the thing I just made / got", not history
    static let maxLimit = 25
    private(set) var limit = maxLimit
    let rules: IgnoreRules
    private let store: String
    private let queue = DispatchQueue(label: "path-shelf")
    private var items: [Item] = []          // newest first, ≤ limit (on `queue`)
    private var loaded = false
    private let snapLock = NSLock()
    private var snapshot: [Item] = []
    private var saveWork: DispatchWorkItem?
    private var notifyWork: DispatchWorkItem?
    // tests: synchronous notification + save
    var immediate = false

    // store / rules are parameters for Tests/test_path_shelf.swift
    init(store: String = NSHomeDirectory() + "/.cache/workspace-switcher/paths.json",
         rules: IgnoreRules = IgnoreRules()) {
        self.store = store
        self.rules = rules
    }

    // [paths] limit (clamped 1...25); loads the store once
    func configure(limit: Int, ignoreFile: String?) {
        queue.sync {
            self.limit = min(max(1, limit), Self.maxLimit)
            rules.shelfFile = ignoreFile
            if !loaded { load(); loaded = true }
            items = Array(items.prefix(self.limit))
            publish()
        }
    }

    // newest first, only paths that still exist (≤ 25 stats)
    func entries() -> [Item] {
        snapLock.lock()
        let all = snapshot
        snapLock.unlock()
        return all.filter { FileManager.default.fileExists(atPath: $0.path) }
    }

    var isEmpty: Bool { queue.sync { items.isEmpty } }

    // file activity (RecentFiles' stream, on its queue): regular files
    // only (no folders, sockets, devices), through the rules
    func observe(_ path: String, created: Bool, origin: String?) {
        queue.async { [self] in
            guard loaded, let c = Self.canonical(path), c.isFile, !rules.ignored(c.path) else { return }
            let why: Why = origin != nil ? .downloaded : created ? .created : .modified
            bump(c.path, why)
        }
    }

    // clipboard / explicit: you asked for these — no rules; an existing
    // file or folder
    func add(_ paths: [String], why: Why) {
        queue.async { [self] in
            guard loaded else { return }
            for p in paths.reversed() {
                guard let c = Self.canonical(Self.normalize(p)), c.isFile || c.isDir else { continue }
                bump(c.path, why)
            }
        }
    }

    // a rename / move (old → new): the row follows, keeps its place
    func renamed(from old: String, to new: String) {
        queue.async { [self] in
            var hit = false
            for i in items.indices {
                if let p = RecentFiles.rekeyed(items[i].path, from: old, to: new) {
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

    // first run (no paths.json yet): the newest of what RecentFiles knows,
    // through the same rules
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

    // tests: wait for the queue
    func sync() { queue.sync {} }

    // the ONE path of a file — symlinked folders resolved (/tmp/link/x and
    // /tmp/x are one row; /tmp is /private/tmp) — and what it is. nil =
    // gone. Shelf rows are always canonical.
    static func canonical(_ p: String) -> (path: String, isFile: Bool, isDir: Bool)? {
        guard let r = realpath(p, nil) else { return nil }
        defer { free(r) }
        let path = String(cString: r)
        var st = stat()
        guard stat(path, &st) == 0 else { return nil }
        let fmt = st.st_mode & S_IFMT
        return (path, fmt == S_IFREG, fmt == S_IFDIR)
    }

    // ~/x, file:///x → /x, standardized; /tmp → /private/tmp (FSEvents'
    // spelling — standardizingPath strips the /private RecentFiles keeps)
    static func normalize(_ p: String) -> String {
        var s = p
        if s.hasPrefix("file://"), let u = URL(string: s), u.isFileURL { s = u.path }
        s = ((s as NSString).expandingTildeInPath as NSString).standardizingPath
        if s == "/tmp" || s.hasPrefix("/tmp/") { s = "/private" + s }
        return s
    }

    // MARK: on `queue`

    private func bump(_ path: String, _ why: Why) {
        let now = Date().timeIntervalSince1970
        if let i = items.firstIndex(where: { $0.path == path }) {
            // an edit of a file you copied / downloaded keeps saying why
            // it is here; a fresh copy or download says so
            var it = items.remove(at: i)
            it.at = now
            if why != .modified { it.why = why }
            items.insert(it, at: 0)
        } else {
            items.insert(Item(path: path, at: now, why: why), at: 0)
        }
        if items.count > limit { items.removeLast(items.count - limit) }
        publish()
    }

    private func dedup() {
        var seen = Set<String>()
        items = items.filter { seen.insert($0.path).inserted }
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
              let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        // stored rows meet today's rules: canonical, still there, and what
        // came from activity still passes the ignore files (an edit to
        // paths.ignore cleans the list on the next launch)
        items = arr.compactMap { d in
            guard let p = d["path"] as? String, let t = d["at"] as? Double, let c = Self.canonical(p) else { return nil }
            let why = Why(rawValue: d["why"] as? String ?? "") ?? .modified
            let activity = [.created, .modified, .downloaded].contains(why)
            guard activity ? c.isFile && !rules.ignored(c.path) : (c.isFile || c.isDir) else { return nil }
            return Item(path: c.path, at: t, why: why)
        }
        items.sort { $0.at > $1.at }
        dedup()
        items = Array(items.sorted { $0.at > $1.at }.prefix(limit))
    }

    private func save() {
        let arr = items.map { ["path": $0.path, "at": $0.at, "why": $0.why.rawValue] as [String: Any] }
        try? FileManager.default.createDirectory(atPath: (store as NSString).deletingLastPathComponent,
                                                 withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: arr) {
            try? data.write(to: URL(fileURLWithPath: store), options: .atomic)
        }
    }
}

// MARK: - Ignore rules (gitignore syntax, git / ripgrep precedence)
//
// Lowest to highest precedence — the LAST matching pattern wins, `!`
// re-includes, and (like git) nothing under an ignored directory can be
// re-included:
//   the global git excludes (core.excludesFile, else ~/.config/git/ignore)
//   per directory, root → the file's folder: .gitignore (only inside a git
//     repo), .ignore, .rgignore — deeper folders win
//   the shelf's own file (config/paths.ignore): your rules always win
// Patterns: `*` `?` `[…]` within a name, `**` across folders, a leading or
// middle `/` anchors to the ignore file's folder (else any depth), a
// trailing `/` = folders only, `\` escapes, `#` comments. In the global and
// shelf files a pattern may also be an absolute or ~/ path.
// Everything is cached per folder; ignore files are re-checked (stat) at
// most every 2 s. Not thread-safe: PathShelf calls it on its queue.
final class IgnoreRules {
    struct Pattern {
        let regex: NSRegularExpression
        let negate: Bool
        let dirOnly: Bool
    }
    // one ignore file: its folder + patterns
    struct RuleSet {
        let base: String        // "/" for the global / shelf files
        let patterns: [Pattern]
        var isGit = false       // a .gitignore: only counts inside a git repo
    }

    var shelfFile: String? { didSet { if shelfFile != oldValue { fileCache[oldValue ?? ""] = nil } } }
    var gitExcludes: String?    // nil = from ~/.gitconfig, else ~/.config/git/ignore
    private let home: String
    private var fileCache: [String: (mtime: Double, checked: Double, set: RuleSet?)] = [:]
    private var dirCache: [String: (checked: Double, repo: Bool, sets: [RuleSet])] = [:]
    var recheck: Double = 2     // seconds between stats of one ignore file (tests: 0)

    init(home: String = NSHomeDirectory(), shelfFile: String? = nil) {
        self.home = home
        self.shelfFile = shelfFile
    }

    // is `path` (absolute) ignored? `isDir` = the path itself is a folder
    func ignored(_ path: String, isDir: Bool = false) -> Bool {
        let comps = (path as NSString).pathComponents   // ["/", "Users", …]
        guard comps.count > 1 else { return false }
        let global = [globalSet()].compactMap { $0 }
        let shelf = [shelfSet()].compactMap { $0 }
        // per-folder sets from "/" down, gathered as we go
        var sets: [RuleSet] = []
        var dir = "/"
        var inRepo = false
        for i in 1..<comps.count {
            let d = folder(dir)
            if d.repo { inRepo = true }
            sets += d.sets.filter { inRepo || !$0.isGit }
            dir = (dir as NSString).appendingPathComponent(comps[i])
            let last = i == comps.count - 1
            // an ignored folder ignores all below it (git never looks inside)
            if decide(dir, isDir: last ? isDir : true, global + sets + shelf) { return true }
        }
        return false
    }

    // last matching pattern wins
    private func decide(_ path: String, isDir: Bool, _ sets: [RuleSet]) -> Bool {
        var ignored = false
        for s in sets {
            guard let rel = Self.relative(path, to: s.base) else { continue }
            let range = NSRange(rel.startIndex..., in: rel)
            for p in s.patterns where !p.dirOnly || isDir {
                if p.regex.firstMatch(in: rel, range: range) != nil { ignored = !p.negate }
            }
        }
        return ignored
    }

    static func relative(_ path: String, to base: String) -> String? {
        if base == "/" { return String(path.dropFirst()) }
        guard path.hasPrefix(base + "/") else { return nil }
        return String(path.dropFirst(base.count + 1))
    }

    // MARK: files

    private func folder(_ dir: String) -> (repo: Bool, sets: [RuleSet]) {
        let now = Date().timeIntervalSince1970
        if let c = dirCache[dir], now - c.checked < recheck { return (c.repo, c.sets) }
        let fm = FileManager.default
        let repo = fm.fileExists(atPath: (dir as NSString).appendingPathComponent(".git"))
        // ripgrep's order inside one folder: .gitignore < .ignore < .rgignore
        var sets: [RuleSet] = []
        for name in [".gitignore", ".ignore", ".rgignore"] {
            let f = (dir as NSString).appendingPathComponent(name)
            if var s = file(f, base: dir) {
                s.isGit = name == ".gitignore"
                sets.append(s)
            }
        }
        dirCache[dir] = (now, repo, sets)
        return (repo, sets)
    }

    private func globalSet() -> RuleSet? {
        file(gitExcludes ?? Self.gitExcludesFile(home: home), base: "/", global: true)
    }

    private func shelfSet() -> RuleSet? {
        guard let f = shelfFile else { return nil }
        return file(f, base: "/", global: true)
    }

    private func file(_ path: String, base: String, global: Bool = false) -> RuleSet? {
        let now = Date().timeIntervalSince1970
        if let c = fileCache[path], now - c.checked < recheck { return c.set }
        var st = stat()
        guard stat(path, &st) == 0 else {
            fileCache[path] = (0, now, nil)
            return nil
        }
        let mt = Double(st.st_mtimespec.tv_sec) + Double(st.st_mtimespec.tv_nsec) / 1e9
        if let c = fileCache[path], c.mtime == mt {
            fileCache[path] = (mt, now, c.set)
            return c.set
        }
        let text = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
        let set = RuleSet(base: base, patterns: text.split(whereSeparator: \.isNewline).compactMap {
            Self.compile(String($0), global: global, home: home)
        })
        fileCache[path] = (mt, now, set)
        return set
    }

    // core.excludesFile from ~/.gitconfig, else $XDG_CONFIG_HOME/git/ignore
    static func gitExcludesFile(home: String) -> String {
        if let text = try? String(contentsOfFile: home + "/.gitconfig", encoding: .utf8) {
            var inCore = false
            for raw in text.split(whereSeparator: \.isNewline) {
                let line = raw.trimmingCharacters(in: .whitespaces)
                if line.hasPrefix("[") { inCore = line.lowercased().hasPrefix("[core") ; continue }
                guard inCore, let eq = line.firstIndex(of: "=") else { continue }
                let key = line[..<eq].trimmingCharacters(in: .whitespaces).lowercased()
                if key == "excludesfile" {
                    var v = line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces)
                    if v.hasPrefix("\"") && v.hasSuffix("\"") && v.count > 1 { v = String(v.dropFirst().dropLast()) }
                    return v.hasPrefix("~/") ? home + v.dropFirst(1) : v
                }
            }
        }
        let xdg = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"] ?? (home + "/.config")
        return xdg + "/git/ignore"
    }

    // MARK: pattern → regex

    // one gitignore line → a pattern matching paths RELATIVE to its base
    static func compile(_ line: String, global: Bool = false, home: String = NSHomeDirectory()) -> Pattern? {
        var s = Substring(line)
        // trailing spaces are dropped unless escaped
        while s.hasSuffix(" ") && !s.hasSuffix("\\ ") { s = s.dropLast() }
        guard !s.isEmpty, !s.hasPrefix("#") else { return nil }
        var negate = false
        if s.hasPrefix("!") { negate = true; s = s.dropFirst() }
        else if s.hasPrefix("\\!") || s.hasPrefix("\\#") { s = s.dropFirst() }
        var dirOnly = false
        if s.hasSuffix("/") && !s.hasSuffix("\\/") { dirOnly = true; s = s.dropLast() }
        guard !s.isEmpty else { return nil }
        var pat = String(s)
        // global / shelf files (base "/"): ~/x is your home, and an absolute
        // path anchors at "/" like any leading "/"
        if global, pat.hasPrefix("~/") { pat = home + pat.dropFirst(1) }
        // a leading or middle "/" anchors to the base; else any depth
        let anchored = pat.contains("/")
        if pat.hasPrefix("/") { pat.removeFirst() }
        var rx = anchored ? "^" : "^(?:.*/)?"
        rx += globToRegex(pat)
        rx += "$"
        guard let re = try? NSRegularExpression(pattern: rx) else { return nil }
        return Pattern(regex: re, negate: negate, dirOnly: dirOnly)
    }

    static func globToRegex(_ glob: String) -> String {
        let c = Array(glob)
        var out = ""
        var i = 0
        while i < c.count {
            let ch = c[i]
            switch ch {
            case "*":
                if i + 1 < c.count, c[i + 1] == "*" {
                    let atStart = i == 0 || c[i - 1] == "/"
                    let atEnd = i + 2 == c.count
                    let slashAfter = i + 2 < c.count && c[i + 2] == "/"
                    if atStart && slashAfter {          // "**/" — zero or more folders
                        out += "(?:.*/)?"
                        i += 3
                        continue
                    }
                    if atStart && atEnd {               // "/**" or "**" — everything inside
                        out += ".*"
                        i += 2
                        continue
                    }
                    out += "[^/]*"                      // "a**b" = two plain stars
                    i += 2
                    continue
                }
                out += "[^/]*"
            case "?":
                out += "[^/]"
            case "[":
                // a character class up to the closing "]" (a "]" first is literal)
                var j = i + 1
                if j < c.count, c[j] == "!" || c[j] == "^" { j += 1 }
                if j < c.count, c[j] == "]" { j += 1 }
                while j < c.count, c[j] != "]" { j += 1 }
                guard j < c.count else { out += "\\["; break }
                var body = String(c[(i + 1)..<j])
                if body.hasPrefix("!") { body = "^" + body.dropFirst() }
                body = body.replacingOccurrences(of: "\\", with: "\\\\")
                out += "[" + body + "]"
                i = j
            case "\\":
                if i + 1 < c.count {
                    out += NSRegularExpression.escapedPattern(for: String(c[i + 1]))
                    i += 1
                } else {
                    out += "\\\\"
                }
            default:
                out += NSRegularExpression.escapedPattern(for: String(ch))
            }
            i += 1
        }
        return out
    }
}

// MARK: - Clipboard paths
//
// Every 0.5 s: the pasteboard's changeCount (one int, nothing parsed unless
// it changed). A change that carries file URLs, or text that is 1-5 lines
// each naming an existing path (absolute, ~/, file://, quoted, `\ `
// escaped), goes to the shelf. Password managers' copies (nspasteboard.org
// concealed / transient / auto-generated markers) and our own copies
// (`ownWrite`) are skipped.
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

    // right after we write the pasteboard ourselves
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

    // text that is NOTHING but 1-5 existing paths, one per line
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
