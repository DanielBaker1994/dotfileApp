import AppKit
import CoreServices

// MARK: - Recent files (the file browser's pinned "Recent" view)
//
// "Where did that download / screenshot / tool output just land?" — without
// a list of folders to maintain. Two sources:
//   live   an FSEvents stream (file-level events) on $HOME and /private/tmp:
//          every file created, renamed in or modified is recorded with the
//          time it happened
//   seed   at launch, Spotlight (mdfind) for what changed in the last
//          `recent-days` while the app wasn't running, plus a shallow scan
//          of /private/tmp (not indexed)
// Noise is filtered out: hidden paths, ~/Library, build / dependency trees,
// in-progress downloads, editor swap files, our own writes (IgnoreSelf),
// plus [files] recent-exclude patterns. The newest `recent-limit` entries
// persist in ~/.cache/workspace-switcher/recent.json.
final class RecentFiles {
    static let shared = RecentFiles()
    static let changed = Notification.Name("RecentFilesChanged")

    private(set) var enabled = false
    private var limit = 200
    private var days = 7
    private var excludes: [String] = []      // user globs (path or name)
    private let queue = DispatchQueue(label: "recent-files")
    private var items: [String: Double] = [:]   // path -> last activity (epoch s)
    private var stream: FSEventStreamRef?
    private var saveWork: DispatchWorkItem?
    private var notifyWork: DispatchWorkItem?
    private let home = NSHomeDirectory()
    private let store = NSHomeDirectory() + "/.cache/workspace-switcher/recent.json"

    // [files] recent / recent-days / recent-limit / recent-exclude
    func configure(enabled on: Bool, days: Int, limit: Int, excludes: [String]) {
        queue.sync {
            self.days = max(1, days)
            self.limit = max(20, limit)
            self.excludes = excludes.map { ($0 as NSString).expandingTildeInPath }
        }
        if on && !enabled { start() } else if !on && enabled { stop() }
    }

    // newest first; only paths that still exist
    func paths() -> [String] {
        queue.sync {
            items.sorted { $0.value > $1.value }.map(\.key)
                .filter { keep($0) && FileManager.default.fileExists(atPath: $0) }
                .prefix(limit).map { $0 }
        }
    }

    func activity(of path: String) -> Date? {
        queue.sync { items[path].map { Date(timeIntervalSince1970: $0) } }
    }

    // MARK: lifecycle

    private func start() {
        enabled = true
        queue.async { [self] in
            load()
            seed()
            publish()
        }
        let ctx = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())
        var context = FSEventStreamContext(version: 0, info: ctx, retain: nil, release: nil,
                                           copyDescription: nil)
        let flags = UInt32(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes
                           | kFSEventStreamCreateFlagNoDefer | kFSEventStreamCreateFlagIgnoreSelf)
        let callback: FSEventStreamCallback = { _, info, count, paths, flags, _ in
            guard let info else { return }
            let me = Unmanaged<RecentFiles>.fromOpaque(info).takeUnretainedValue()
            let arr = unsafeBitCast(paths, to: NSArray.self) as? [String] ?? []
            me.handle(arr, Array(UnsafeBufferPointer(start: flags, count: count)))
        }
        guard let s = FSEventStreamCreate(nil, callback, &context,
                                          [home, "/private/tmp"] as CFArray,
                                          FSEventStreamEventId(kFSEventStreamEventIdSinceNow),
                                          0.5, FSEventStreamCreateFlags(flags)) else { return }
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

    // on `queue`
    private func handle(_ paths: [String], _ flags: [FSEventStreamEventFlags]) {
        let now = Date().timeIntervalSince1970
        var touched = false
        for (i, raw) in paths.enumerated() where i < flags.count {
            let f = Int(flags[i])
            let p = raw.hasPrefix("/tmp/") ? "/private" + raw : raw
            if f & kFSEventStreamEventFlagItemRemoved != 0, !FileManager.default.fileExists(atPath: p) {
                if items.removeValue(forKey: p) != nil { touched = true }
                continue
            }
            let isFile = f & kFSEventStreamEventFlagItemIsFile != 0
            let isDir = f & kFSEventStreamEventFlagItemIsDir != 0
            let created = f & kFSEventStreamEventFlagItemCreated != 0
            let renamed = f & kFSEventStreamEventFlagItemRenamed != 0
            let modified = f & kFSEventStreamEventFlagItemModified != 0
            // files that appear or change; folders only when they appear
            // (an unzipped archive, a new project)
            guard (isFile && (created || renamed || modified)) || (isDir && (created || renamed)),
                  keep(p), FileManager.default.fileExists(atPath: p) else { continue }
            items[p] = now
            touched = true
        }
        if touched { trim(); publish() }
    }

    // MARK: seed (what happened while the app wasn't running)

    private func seed() {
        let since = Date().timeIntervalSince1970 - Double(days) * 86400
        var found: [String: Double] = [:]
        let secs = days * 86400
        let q = "kMDItemFSContentChangeDate >= $time.now(-\(secs)) || kMDItemDateAdded >= $time.now(-\(secs))"
        for p in run("/usr/bin/mdfind", ["-onlyin", home, q]) where keep(p) {
            if let t = stamp(p), t >= since { found[p] = t }
        }
        // /private/tmp: not Spotlight-indexed — two levels deep
        let fm = FileManager.default
        func scan(_ dir: String, depth: Int) {
            for n in (try? fm.contentsOfDirectory(atPath: dir)) ?? [] {
                let p = (dir as NSString).appendingPathComponent(n)
                guard keep(p) else { continue }
                var isDir: ObjCBool = false
                guard fm.fileExists(atPath: p, isDirectory: &isDir) else { continue }
                if let t = stamp(p), t >= since, !isDir.boolValue { found[p] = t }
                if isDir.boolValue && depth > 0 { scan(p, depth: depth - 1) }
            }
        }
        scan("/private/tmp", depth: 1)
        for (p, t) in found where (items[p] ?? 0) < t { items[p] = t }
        trim()
    }

    // newest of creation / modification
    private func stamp(_ p: String) -> Double? {
        var st = Darwin.stat()
        guard stat(p, &st) == 0 else { return nil }
        return max(Double(st.st_birthtimespec.tv_sec), Double(st.st_mtimespec.tv_sec))
    }

    private func run(_ exe: String, _ args: [String]) -> [String] {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: exe)
        p.arguments = args
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        do { try p.run() } catch { return [] }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return String(decoding: data, as: UTF8.self).split(separator: "\n").map(String.init)
    }

    // MARK: noise filter

    private static let noiseDirs: Set<String> = [
        "node_modules", "DerivedData", "__pycache__", "site-packages", "Pods", "venv",
        "Caches", "CachedData", "logs", "xcuserdata",
    ]
    // app libraries / bundles are packages whose insides churn constantly
    // (Photos' database, Music's library) — and touching the Photos or Music
    // library triggers a privacy prompt. Never look inside one.
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
        if p.hasPrefix(home + "/Library/") || p == home + "/Library" { return false }
        let comps = (p as NSString).pathComponents
        for c in comps.dropFirst() {
            if c.hasPrefix(".") || Self.noiseDirs.contains(c) || Self.packageNames.contains(c) { return false }
            if Self.packageExts.contains((c as NSString).pathExtension.lowercased()) { return false }
        }
        // ~/Music/Music = the Music app's media library (Media Library prompt)
        if p.hasPrefix(home + "/Music/Music/") { return false }
        let name = comps.last ?? ""
        let ext = (name as NSString).pathExtension.lowercased()
        if Self.noiseExts.contains(ext) || name.hasSuffix("~") || name == "4913" { return false }
        // /private/tmp: skip system / tool scratch areas
        if p.hasPrefix("/private/tmp/") {
            let top = comps.count > 3 ? comps[3] : ""
            if top.hasPrefix("com.apple") || top.hasPrefix("claude") || top.hasPrefix("tmp")
                || top.hasPrefix("ws-") || top.hasPrefix("workspace-switcher")
                || top.hasPrefix("jira-poll") { return false }   // our own agents' logs
        }
        for g in excludes where !g.isEmpty {
            if fnmatch(g, p, 0) == 0 || fnmatch(g, name, 0) == 0 || p.hasPrefix(g) { return false }
        }
        return true
    }

    // MARK: store

    private func trim() {
        guard items.count > limit * 2 else { return }
        let keepPaths = items.sorted { $0.value > $1.value }.prefix(limit * 2)
        items = Dictionary(uniqueKeysWithValues: keepPaths.map { ($0.key, $0.value) })
    }

    private func load() {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: store)),
              let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        let since = Date().timeIntervalSince1970 - Double(days) * 86400
        for d in arr {
            if let p = d["path"] as? String, let t = d["at"] as? Double, t >= since, keep(p) {
                items[p] = max(items[p] ?? 0, t)
            }
        }
    }

    // on `queue`: tell the browsers (debounced) and save (debounced)
    private func publish() {
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
        let arr = items.sorted { $0.value > $1.value }.prefix(limit).map { ["path": $0.key, "at": $0.value] }
        try? FileManager.default.createDirectory(atPath: (store as NSString).deletingLastPathComponent,
                                                 withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: Array(arr)) {
            try? data.write(to: URL(fileURLWithPath: store), options: .atomic)
        }
    }
}
