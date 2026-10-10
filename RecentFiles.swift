import AppKit
import CoreServices

final class RecentFiles {
    static let shared = RecentFiles()
    static let changed = Notification.Name("RecentFilesChanged")

    struct Item {
        var at: Double
        var source: String?
    }

    private(set) var enabled = false
    private var limit = 200
    private var days = 7
    private var everywhere = true
    private var excludes: [String] = []
    private let queue = DispatchQueue(label: "recent-files")
    private var items: [String: Item] = [:]
    private var stream: FSEventStreamRef?
    private var saveWork: DispatchWorkItem?
    private var notifyWork: DispatchWorkItem?
    private let snapLock = NSLock()
    private var snapshot: [(path: String, at: Date, source: String?)] = []
    private let home: String
    private let store: String
    private var departed: [UInt64: (at: Double, items: [String: Item], path: String)] = [:]
    var onKept: ((_ path: String, _ created: Bool, _ origin: String?) -> Void)?
    var onRenamed: ((_ from: String, _ to: String) -> Void)?

    init(home: String = NSHomeDirectory(),
         store: String = NSHomeDirectory() + "/.cache/kitchen-sink/recent.json") {
        self.home = home
        self.store = store
    }

    func configure(enabled on: Bool, days: Int, limit: Int, excludes: [String], everywhere all: Bool) {
        let rescope = enabled && all != everywhere
        queue.sync {
            self.days = max(1, days)
            self.limit = max(20, limit)
            self.everywhere = all
            self.excludes = excludes.map { ($0 as NSString).expandingTildeInPath }
        }
        if rescope { stop() }
        if on && !enabled { start() } else if !on && enabled { stop() }
        if enabled { queue.async { [self] in publish() } }
    }

    func entries(arrivedOnly: Bool = false) -> [(path: String, at: Date, source: String?)] {
        snapLock.lock()
        let all = snapshot
        snapLock.unlock()
        return Array(all.filter { !arrivedOnly || $0.source != nil }.prefix(limit))
    }

    func ownChange(from old: String?, to new: String) {
        guard enabled else { return }
        if let old { onRenamed?(old, new) } else { onKept?(new, true, nil) }
        if let old {
            snapLock.lock()
            snapshot = snapshot.compactMap { e in
                guard let p = Self.rekeyed(e.path, from: old, to: new) else { return e }
                return keep(p) ? (path: p, at: e.at, source: e.source) : nil
            }
            snapLock.unlock()
        }
        queue.async { [self] in
            let carried = old.map { rekey(from: $0, to: new) } ?? false
            if !carried, keep(new), FileManager.default.fileExists(atPath: new) {
                items[new] = Item(at: Date().timeIntervalSince1970, source: Self.origin(new))
            }
            trim()
            publish()
        }
    }

    static func present(_ p: String) -> Bool {
        guard FileManager.default.fileExists(atPath: p) else { return false }
        let real = try? URL(fileURLWithPath: p).resourceValues(forKeys: [.nameKey]).name
        return real == nil || real == (p as NSString).lastPathComponent
    }

    static func rekeyed(_ p: String, from old: String, to new: String) -> String? {
        if p == old { return new }
        if p.hasPrefix(old + "/") { return new + p.dropFirst(old.count) }
        return nil
    }

    private func rekey(from old: String, to new: String) -> Bool {
        let gone = take(old)
        for (suffix, it) in gone { put(new + suffix, it) }
        return !gone.isEmpty
    }

    private func take(_ p: String) -> [String: Item] {
        var gone: [String: Item] = [:]
        for (k, it) in items where k == p || k.hasPrefix(p + "/") {
            gone[String(k.dropFirst(p.count))] = it
        }
        for suffix in gone.keys { items[p + suffix] = nil }
        return gone
    }

    private func put(_ p: String, _ it: Item) {
        guard let have = items[p] else { items[p] = it; return }
        items[p] = Item(at: max(have.at, it.at), source: have.source ?? it.source)
    }

    private func start() {
        enabled = true
        queue.async { [self] in
            for d in ["Downloads", "Desktop", "Documents"] {
                _ = try? FileManager.default.contentsOfDirectory(atPath: home + "/" + d)
            }
            load()
            seed()
            for (p, it) in items where it.source == nil {
                if let o = Self.origin(p) { items[p]?.source = o }
            }
            publish()
        }
        let ctx = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())
        var context = FSEventStreamContext(version: 0, info: ctx, retain: nil, release: nil,
                                           copyDescription: nil)
        let flags = UInt32(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes
                           | kFSEventStreamCreateFlagUseExtendedData
                           | kFSEventStreamCreateFlagIgnoreSelf)
        let callback: FSEventStreamCallback = { _, info, count, paths, flags, _ in
            guard let info else { return }
            let me = Unmanaged<RecentFiles>.fromOpaque(info).takeUnretainedValue()
            let arr = unsafeBitCast(paths, to: NSArray.self) as? [NSDictionary] ?? []
            me.handle(arr.map { $0["path"] as? String ?? "" },
                      Array(UnsafeBufferPointer(start: flags, count: count)),
                      arr.map { ($0["fileID"] as? NSNumber)?.uint64Value })
        }
        let roots = queue.sync { everywhere } ? ["/"] : [home, "/private/tmp"]
        guard let s = FSEventStreamCreate(nil, callback, &context, roots as CFArray,
                                          FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
                                          1.0, FSEventStreamCreateFlags(flags)) else { return }
        FSEventStreamSetDispatchQueue(s, queue)
        FSEventStreamStart(s)
        stream = s
    }

    private func stop() {
        enabled = false
        if let s = stream {
            FSEventStreamStop(s)
            FSEventStreamInvalidate(s)
            FSEventStreamRelease(s)
        }
        stream = nil
    }

    func handle(_ paths: [String], _ flags: [FSEventStreamEventFlags], _ ids: [UInt64?] = []) {
        let now = Date().timeIntervalSince1970
        var touched = false
        departed = departed.filter { now - $0.value.at < 10 }
        for (i, raw) in paths.enumerated() where i < flags.count {
            let p = raw.hasPrefix("/tmp/") ? "/private" + raw : raw
            guard inScope(p) else { continue }
            let f = Int(flags[i])
            let id = i < ids.count ? ids[i] : nil
            if f & (kFSEventStreamEventFlagItemRemoved | kFSEventStreamEventFlagItemRenamed) != 0,
               !Self.present(p) {
                let gone = take(p)
                if !gone.isEmpty { touched = true }
                if let id, f & kFSEventStreamEventFlagItemRenamed != 0 { departed[id] = (now, gone, p) }
                continue
            }
            let isFile = f & kFSEventStreamEventFlagItemIsFile != 0
            let isDir = f & kFSEventStreamEventFlagItemIsDir != 0
            let created = f & kFSEventStreamEventFlagItemCreated != 0
            let renamed = f & kFSEventStreamEventFlagItemRenamed != 0
            let modified = f & kFSEventStreamEventFlagItemModified != 0
            guard (isFile && (created || renamed || modified)) || (isDir && (created || renamed)),
                  keep(p), FileManager.default.fileExists(atPath: p) else { continue }
            var item = items[p] ?? Item(at: now, source: nil)
            item.at = now
            if created || renamed || item.source == nil { item.source = Self.origin(p) ?? item.source }
            var renamedFrom: String?
            if renamed, let id, let was = departed.removeValue(forKey: id) {
                item.source = item.source ?? was.items[""]?.source
                for (suffix, it) in was.items where !suffix.isEmpty { put(p + suffix, it) }
                renamedFrom = was.path
            }
            items[p] = item
            touched = true
            if let from = renamedFrom { onRenamed?(from, p) }
            if isFile { onKept?(p, created, item.source) }
        }
        if touched { trim(); publish() }
    }

    static func origin(_ p: String) -> String? {
        guard let q = xattrString(p, "com.apple.quarantine") else { return nil }
        let parts = q.split(separator: ";", omittingEmptySubsequences: false).map(String.init)
        var agent = parts.count > 2 ? parts[2] : ""
        switch agent.lowercased() {
        case "sharingd", "airdrop": agent = "AirDrop"
        case "": agent = "downloaded"
        default: break
        }
        if let host = whereFromHost(p), agent != "AirDrop" { return "\(agent) · \(host)" }
        return agent
    }

    private static func xattrData(_ p: String, _ name: String) -> Data? {
        let n = getxattr(p, name, nil, 0, 0, XATTR_NOFOLLOW)
        guard n > 0 else { return nil }
        var buf = [UInt8](repeating: 0, count: n)
        guard getxattr(p, name, &buf, n, 0, XATTR_NOFOLLOW) == n else { return nil }
        return Data(buf)
    }

    private static func xattrString(_ p: String, _ name: String) -> String? {
        xattrData(p, name).map { String(decoding: $0, as: UTF8.self) }
    }

    private static func whereFromHost(_ p: String) -> String? {
        guard let d = xattrData(p, "com.apple.metadata:kMDItemWhereFroms"),
              let arr = try? PropertyListSerialization.propertyList(from: d, format: nil) as? [String],
              let first = arr.first(where: { $0.hasPrefix("http") }), let host = URL(string: first)?.host
        else { return nil }
        return host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
    }

    private func seed() {
        let since = Date().timeIntervalSince1970 - Double(days) * 86400
        var found: [String: Double] = [:]
        let secs = days * 86400
        let changed = "kMDItemFSContentChangeDate >= $time.now(-\(secs)) || kMDItemDateAdded >= $time.now(-\(secs))"
        for p in run("/usr/bin/mdfind", ["-onlyin", home, changed]) where keep(p) {
            if let t = stamp(p), t >= since { found[Self.canonical(p)] = t }
        }
        if everywhere {
            let downloaded = "kMDItemDateAdded >= $time.now(-\(secs)) && kMDItemWhereFroms == \"*\""
            for p in run("/usr/bin/mdfind", [downloaded]) where inScope(p) && keep(p) {
                if let t = stamp(p), t >= since { found[Self.canonical(p)] = t }
            }
        }
        let fm = FileManager.default
        func scan(_ dir: String, depth: Int) {
            for n in (try? fm.contentsOfDirectory(atPath: dir)) ?? [] {
                let p = (dir as NSString).appendingPathComponent(n)
                guard keep(p) else { continue }
                var ls = Darwin.stat()
                guard lstat(p, &ls) == 0, ls.st_mode & S_IFMT != S_IFLNK else { continue }
                var isDir: ObjCBool = false
                guard fm.fileExists(atPath: p, isDirectory: &isDir) else { continue }
                if let t = stamp(p), t >= since, !isDir.boolValue { found[p] = t }
                if isDir.boolValue && depth > 0 { scan(p, depth: depth - 1) }
            }
        }
        scan("/private/tmp", depth: 1)
        for (p, t) in found where (items[p]?.at ?? 0) < t {
            items[p] = Item(at: t, source: items[p]?.source ?? Self.origin(p))
        }
        trim()
    }

    static func canonical(_ p: String) -> String {
        let dir = (p as NSString).deletingLastPathComponent
        guard let r = realpath(dir, nil) else { return p }
        defer { free(r) }
        let real = String(cString: r)
        return real == dir ? p : (real as NSString).appendingPathComponent((p as NSString).lastPathComponent)
    }

    private func stamp(_ p: String) -> Double? {
        var st = Darwin.stat()
        guard stat(p, &st) == 0 else { return nil }
        return max(Double(st.st_birthtimespec.tv_sec), Double(st.st_mtimespec.tv_sec))
    }

    private func run(_ exe: String, _ args: [String]) -> [String] {
        ((try? runProcess(exe, args))?.out ?? "").split(separator: "\n").map(String.init)
    }

    private static let systemRoots = [
        "/System/", "/Library/", "/private/var/", "/private/etc/", "/var/", "/etc/", "/usr/",
        "/bin/", "/sbin/", "/opt/", "/cores/", "/dev/", "/Volumes/", "/Applications/", "/nix/",
        "/private/preboot/", "/private/xarts/", "/.",
    ]

    private func inScope(_ p: String) -> Bool {
        if p.hasPrefix(home + "/") { return !p.hasPrefix(home + "/Library/") }
        if p.hasPrefix("/private/tmp/") { return true }
        if !everywhere { return false }
        if p.hasPrefix("/Users/") { return p.hasPrefix("/Users/Shared/") }
        for r in Self.systemRoots where p.hasPrefix(r) { return false }
        return true
    }

    private static let noiseDirs: Set<String> = [
        "node_modules", "DerivedData", "__pycache__", "site-packages", "Pods", "venv",
        "Caches", "CachedData", "logs", "xcuserdata",
    ]
    private static let packageExts: Set<String> = [
        "photoslibrary", "photolibrary", "migratedphotolibrary", "aplibrary", "musiclibrary",
        "tvlibrary", "app", "bundle", "framework", "plugin", "kext", "xcarchive", "xcodeproj",
        "xcworkspace", "playground", "sparsebundle", "photobooth",
    ]
    private static let packageNames: Set<String> = ["Photo Booth Library"]

    private static let noiseExts: Set<String> = [
        "crdownload", "part", "download", "partial", "swp", "swo", "swx", "tmp", "lock",
        "pid", "sock", "socket", "db-journal", "db-wal", "db-shm", "sqlite-journal",
        "sqlite-wal", "sqlite-shm",
    ]

    private func keep(_ p: String) -> Bool {
        guard inScope(p) else { return false }
        let comps = (p as NSString).pathComponents
        for c in comps.dropFirst() {
            if c.hasPrefix(".") || Self.noiseDirs.contains(c) || Self.packageNames.contains(c) { return false }
            if Self.packageExts.contains((c as NSString).pathExtension.lowercased()) { return false }
        }
        if p.hasPrefix(home + "/Music/Music/") { return false }
        let name = comps.last ?? ""
        let ext = (name as NSString).pathExtension.lowercased()
        if Self.noiseExts.contains(ext) || name.hasSuffix("~") || name == "4913" { return false }
        if p.hasPrefix("/private/tmp/"), !p.hasPrefix(home + "/") {
            let top = comps.count > 3 ? comps[3] : ""
            if top.hasPrefix("com.apple") || top.hasPrefix("claude") || top.hasPrefix("tmp")
                || top.hasPrefix("ws-") || top.hasPrefix("kitchen-sink")
                || top.hasPrefix("jira-poll") { return false }
        }
        for g in excludes where !g.isEmpty {
            if fnmatch(g, p, 0) == 0 || fnmatch(g, name, 0) == 0 || p.hasPrefix(g) { return false }
        }
        return true
    }

    private func trim() {
        guard items.count > limit * 2 else { return }
        let keepPaths = items.sorted { $0.value.at > $1.value.at }.prefix(limit * 2)
        items = Dictionary(uniqueKeysWithValues: keepPaths.map { ($0.key, $0.value) })
    }

    private func load() {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: store)),
              let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        let since = Date().timeIntervalSince1970 - Double(days) * 86400
        for d in arr {
            if let raw = d["path"] as? String, case let p = Self.canonical(raw), let t = d["at"] as? Double,
               t >= since, keep(p), t > (items[p]?.at ?? 0) {
                items[p] = Item(at: t, source: d["source"] as? String)
            }
        }
    }

    private func publish() {
        let snap = items.sorted { $0.value.at > $1.value.at }
            .filter { keep($0.key) && FileManager.default.fileExists(atPath: $0.key) }
            .map { ($0.key, Date(timeIntervalSince1970: $0.value.at), $0.value.source) }
        snapLock.lock()
        snapshot = snap
        snapLock.unlock()
        notifyWork?.cancel()
        let n = DispatchWorkItem { NotificationCenter.default.post(name: Self.changed, object: nil) }
        notifyWork = n
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: n)
        saveWork?.cancel()
        let s = DispatchWorkItem { [weak self] in self?.save() }
        saveWork = s
        queue.asyncAfter(deadline: .now() + 2, execute: s)
    }

    private func save() {
        let arr = items.sorted { $0.value.at > $1.value.at }.prefix(limit).map { kv -> [String: Any] in
            var d: [String: Any] = ["path": kv.key, "at": kv.value.at]
            if let s = kv.value.source { d["source"] = s }
            return d
        }
        try? FileManager.default.createDirectory(atPath: (store as NSString).deletingLastPathComponent,
                                                 withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: Array(arr)) {
            try? data.write(to: URL(fileURLWithPath: store), options: .atomic)
        }
    }
}
