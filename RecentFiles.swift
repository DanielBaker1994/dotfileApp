import AppKit
import CoreServices

// MARK: - Recent files (the file browser's pinned "Recent" + "Arrived" views)
//
// "Where did that download / AirDrop / scp / screenshot just land?" — with
// no list of folders to maintain:
//   live   ONE FSEvents stream on "/" (file-level events; the kernel journal
//          makes this cheap). System trees are dropped by a prefix check
//          before any syscall; what's left — your home, /tmp, /Users/Shared,
//          anywhere else you can write — is recorded with the time it
//          happened. [files] recent-scope = home watches only ~ and /tmp.
//   origin a file created / renamed in carrying macOS's quarantine flag came
//          from OUTSIDE: a browser download, AirDrop, Messages, Mail — the
//          "Arrived" view lists only those, wherever they were saved, with
//          the app (and site) they came from. (scp / curl set no flag: those
//          show in Recent.)
//   seed   at launch, Spotlight for what changed in ~ in the last
//          `recent-days`, every downloaded file on the disk (kMDItemWhereFroms)
//          in that time, and a shallow scan of /tmp (not indexed).
// Noise is skipped: system trees, hidden paths, ~/Library, app libraries
// (Photos / Music — touching them triggers privacy prompts), build and
// dependency trees, in-progress downloads, swap files, our own writes
// (IgnoreSelf), plus [files] recent-exclude. Stored in
// ~/.cache/workspace-switcher/recent.json.
final class RecentFiles {
    static let shared = RecentFiles()
    static let changed = Notification.Name("RecentFilesChanged")

    struct Item {
        var at: Double          // last activity (epoch s)
        var source: String?     // where it came from ("Safari · github.com", "AirDrop")
    }

    private(set) var enabled = false
    private var limit = 200
    private var days = 7
    private var everywhere = true
    private var excludes: [String] = []      // user globs (path or name)
    private let queue = DispatchQueue(label: "recent-files")
    private var items: [String: Item] = [:]
    private var stream: FSEventStreamRef?
    private var saveWork: DispatchWorkItem?
    private var notifyWork: DispatchWorkItem?
    // what entries() hands out: rebuilt on `queue` by publish(), read under
    // a lock — the main thread never waits on `queue` (busy with a burst of
    // file events or the launch-time Spotlight seed, it stalled every
    // arrow press in the Recent view)
    private let snapLock = NSLock()
    private var snapshot: [(path: String, at: Date, source: String?)] = []
    private let home = NSHomeDirectory()
    private let store = NSHomeDirectory() + "/.cache/workspace-switcher/recent.json"

    // [files] recent / recent-days / recent-limit / recent-exclude / recent-scope
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
        // excludes / limit may have changed what the snapshot holds
        if enabled { queue.async { [self] in publish() } }
    }

    // newest first; only paths that still exist. arrivedOnly: files that
    // came from outside (quarantine flag)
    func entries(arrivedOnly: Bool = false) -> [(path: String, at: Date, source: String?)] {
        snapLock.lock()
        let all = snapshot
        snapLock.unlock()
        return Array(all.filter { !arrivedOnly || $0.source != nil }.prefix(limit))
    }

    // MARK: lifecycle

    private func start() {
        enabled = true
        queue.async { [self] in
            // ask for the protected folders UP FRONT, at launch: if a grant
            // is missing, macOS prompts now (nothing on screen to disturb)
            // instead of mid-use when a row is listed. Normally the build
            // pre-grants them (bin/grant-permissions.sh) and this is silent.
            for d in ["Downloads", "Desktop", "Documents"] {
                _ = try? FileManager.default.contentsOfDirectory(atPath: home + "/" + d)
            }
            load()
            seed()
            // entries stored before origins were tracked: read them once
            for (p, it) in items where it.source == nil {
                if let o = Self.origin(p) { items[p]?.source = o }
            }
            publish()
        }
        let ctx = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())
        var context = FSEventStreamContext(version: 0, info: ctx, retain: nil, release: nil,
                                           copyDescription: nil)
        let flags = UInt32(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes
                           | kFSEventStreamCreateFlagIgnoreSelf)
        let callback: FSEventStreamCallback = { _, info, count, paths, flags, _ in
            guard let info else { return }
            let me = Unmanaged<RecentFiles>.fromOpaque(info).takeUnretainedValue()
            let arr = unsafeBitCast(paths, to: NSArray.self) as? [String] ?? []
            me.handle(arr, Array(UnsafeBufferPointer(start: flags, count: count)))
        }
        let roots = queue.sync { everywhere } ? ["/"] : [home, "/private/tmp"]
        // 1s latency: the kernel batches a busy disk into one callback
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

    // on `queue`
    private func handle(_ paths: [String], _ flags: [FSEventStreamEventFlags]) {
        let now = Date().timeIntervalSince1970
        var touched = false
        for (i, raw) in paths.enumerated() where i < flags.count {
            let p = raw.hasPrefix("/tmp/") ? "/private" + raw : raw
            // the cheap string check first: a busy disk sends thousands of
            // system-tree events that must cost nothing
            guard inScope(p) else { continue }
            let f = Int(flags[i])
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
            // (an unzipped archive, an AirDropped folder)
            guard (isFile && (created || renamed || modified)) || (isDir && (created || renamed)),
                  keep(p), FileManager.default.fileExists(atPath: p) else { continue }
            var item = items[p] ?? Item(at: now, source: nil)
            item.at = now
            // a download finishes by RENAMING foo.crdownload -> foo: read
            // the origin when the file appears under its final name
            if created || renamed || item.source == nil { item.source = Self.origin(p) ?? item.source }
            items[p] = item
            touched = true
        }
        if touched { trim(); publish() }
    }

    // MARK: origin (quarantine + where-from)

    // "Safari · github.com", "AirDrop", "Messages" — nil when the file carries
    // no quarantine flag (made here, or copied in by scp / cp / curl)
    static func origin(_ p: String) -> String? {
        guard let q = xattrString(p, "com.apple.quarantine") else { return nil }
        // flags;hex-time;agent;uuid
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

    // the site a download came from (kMDItemWhereFroms: [url, referrer])
    private static func whereFromHost(_ p: String) -> String? {
        guard let d = xattrData(p, "com.apple.metadata:kMDItemWhereFroms"),
              let arr = try? PropertyListSerialization.propertyList(from: d, format: nil) as? [String],
              let first = arr.first(where: { $0.hasPrefix("http") }), let host = URL(string: first)?.host
        else { return nil }
        return host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
    }

    // MARK: seed (what happened while the app wasn't running)

    private func seed() {
        let since = Date().timeIntervalSince1970 - Double(days) * 86400
        var found: [String: Double] = [:]
        let secs = days * 86400
        let changed = "kMDItemFSContentChangeDate >= $time.now(-\(secs)) || kMDItemDateAdded >= $time.now(-\(secs))"
        for p in run("/usr/bin/mdfind", ["-onlyin", home, changed]) where keep(p) {
            if let t = stamp(p), t >= since { found[p] = t }
        }
        // every file DOWNLOADED in that time, wherever it was saved
        if everywhere {
            let downloaded = "kMDItemDateAdded >= $time.now(-\(secs)) && kMDItemWhereFroms == \"*\""
            for p in run("/usr/bin/mdfind", [downloaded]) where inScope(p) && keep(p) {
                if let t = stamp(p), t >= since { found[p] = t }
            }
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
        for (p, t) in found where (items[p]?.at ?? 0) < t {
            items[p] = Item(at: t, source: items[p]?.source ?? Self.origin(p))
        }
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

    // system trees (and other volumes: network / removable drives prompt
    // for access) — never user drop zones
    private static let systemRoots = [
        "/System/", "/Library/", "/private/var/", "/private/etc/", "/var/", "/etc/", "/usr/",
        "/bin/", "/sbin/", "/opt/", "/cores/", "/dev/", "/Volumes/", "/Applications/", "/nix/",
        "/private/preboot/", "/private/xarts/", "/.",
    ]

    // where a user file can land: not a system tree, and under /Users only
    // your home + /Users/Shared
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
        guard inScope(p) else { return false }
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
        let keepPaths = items.sorted { $0.value.at > $1.value.at }.prefix(limit * 2)
        items = Dictionary(uniqueKeysWithValues: keepPaths.map { ($0.key, $0.value) })
    }

    private func load() {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: store)),
              let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
        let since = Date().timeIntervalSince1970 - Double(days) * 86400
        for d in arr {
            if let p = d["path"] as? String, let t = d["at"] as? Double, t >= since, keep(p),
               t > (items[p]?.at ?? 0) {
                items[p] = Item(at: t, source: d["source"] as? String)
            }
        }
    }

    // on `queue`: rebuild the snapshot, tell the browsers (debounced) and
    // save (debounced)
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
