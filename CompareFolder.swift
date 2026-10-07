import Foundation

enum FolderStatus: String {
    case same, different, unimportant, leftOnly, rightOnly, unknown, error
    var isOrphan: Bool { self == .leftOnly || self == .rightOnly }
    var isDiff: Bool { self != .same }
}

enum FolderNewer: String { case none, left, right }

enum FolderFilter: String, CaseIterable {
    case all, diffs, same, orphans, leftNewer, rightNewer
    var title: String {
        switch self {
        case .all: return "All"
        case .diffs: return "Differences"
        case .same: return "Same"
        case .orphans: return "Orphans"
        case .leftNewer: return "Left newer"
        case .rightNewer: return "Right newer"
        }
    }
}

struct FolderSideInfo {
    var name: String
    var isDir: Bool
    var isLink: Bool
    var link: String?
    var size: Int64
    var mtime: Double
    var targetSize: Int64? = nil
    var targetMtime: Double = 0

    var asFile: (size: Int64, mtime: Double)? {
        if isLink { return targetSize.map { ($0, targetMtime) } }
        return isDir ? nil : (size, mtime)
    }

    var json: [String: Any] {
        ["name": name, "isDir": isDir, "isLink": isLink,
         "link": link ?? NSNull(), "size": size, "mtime": mtime,
         "targetSize": targetSize ?? NSNull(), "targetMtime": targetMtime]
    }

    init(name: String, isDir: Bool, isLink: Bool, link: String?, size: Int64, mtime: Double,
         targetSize: Int64? = nil, targetMtime: Double = 0) {
        self.name = name
        self.isDir = isDir
        self.isLink = isLink
        self.link = link
        self.size = size
        self.mtime = mtime
        self.targetSize = targetSize
        self.targetMtime = targetMtime
    }

    init(json: [String: Any]) {
        name = json["name"] as? String ?? ""
        isDir = json["isDir"] as? Bool ?? false
        isLink = json["isLink"] as? Bool ?? false
        link = json["link"] as? String
        size = (json["size"] as? NSNumber)?.int64Value ?? 0
        mtime = (json["mtime"] as? NSNumber)?.doubleValue ?? 0
        targetSize = (json["targetSize"] as? NSNumber)?.int64Value
        targetMtime = (json["targetMtime"] as? NSNumber)?.doubleValue ?? 0
    }
}

final class FolderNode {
    let id: Int
    let key: String
    let rel: String
    var name: String
    var left: FolderSideInfo?
    var right: FolderSideInfo?
    var status: FolderStatus = .same
    var sameByMetadata = false
    var newer: FolderNewer = .none
    var children: [FolderNode] = []
    weak var parent: FolderNode?
    var expanded = false
    var depth = 0
    var diffBelow = 0, importantBelow = 0, unknownBelow = 0

    init(id: Int, key: String, rel: String, name: String) {
        self.id = id
        self.key = key
        self.rel = rel
        self.name = name
    }

    var isDir: Bool { (left?.isDir ?? false) || (right?.isDir ?? false) }
    var kindMismatch: Bool {
        if let l = left, let r = right { return l.isDir != r.isDir }
        return false
    }
}

struct FolderOptions {
    var timeTolerance: Double = 2
    var content = "auto"
    var hidden = true
    var exclude: [String] = []
    var importance = Importance()
    var useGitignore = false
    var ignoreFile = ""
    var recheck: Double = 30

    var json: [String: Any] {
        ["timeTolerance": timeTolerance, "content": content, "hidden": hidden, "exclude": exclude,
         "useGitignore": useGitignore, "ignoreFile": ignoreFile, "recheck": recheck]
    }
}

private func cfBox(_ method: String, _ params: [String: Any],
                   timeout: TimeInterval = 600) -> [String: Any]? {
    guard case .success(let box) = PythonHelper.shared.callSync(method, params, timeout: timeout),
          let dict = box as? [String: Any] else { return nil }
    return dict
}

final class FolderTree {
    let leftRoot: String
    let rightRoot: String
    let handle: Int
    var roots: [FolderNode] = []
    private(set) var all: [FolderNode] = []
    var caseInsensitive = true
    var truncated = false
    var errors: [String] = []

    init(left: String, right: String, handle: Int) {
        leftRoot = left
        rightRoot = right
        self.handle = handle
    }

    deinit {
        if handle >= 0 { _ = cfBox("folder.drop", ["handle": handle]) }
    }

    func adopt(_ n: FolderNode) { all.append(n) }
    func node(_ id: Int) -> FolderNode? { id >= 0 && id < all.count && all[id].id == id ? all[id] : nil }

    func path(_ n: FolderNode, _ side: CompareSide) -> String {
        cfBox("folder.path", ["handle": handle, "id": n.id, "side": side.rawValue])?["path"] as? String
            ?? n.rel
    }

    var pending: [FolderNode] {
        (cfBox("folder.pending", ["handle": handle])?["ids"] as? [Int] ?? []).compactMap(node)
    }

    struct Counts {
        var different = 0, unimportant = 0, leftOnly = 0, rightOnly = 0
        var same = 0, sameByMetadata = 0, unknown = 0, error = 0
    }

    func counts() -> Counts {
        var c = Counts()
        guard let d = cfBox("folder.counts", ["handle": handle]) else { return c }
        c.different = d["different"] as? Int ?? 0
        c.unimportant = d["unimportant"] as? Int ?? 0
        c.leftOnly = d["leftOnly"] as? Int ?? 0
        c.rightOnly = d["rightOnly"] as? Int ?? 0
        c.same = d["same"] as? Int ?? 0
        c.sameByMetadata = d["sameByMetadata"] as? Int ?? 0
        c.unknown = d["unknown"] as? Int ?? 0
        c.error = d["error"] as? Int ?? 0
        return c
    }

    struct Row {
        let node: FolderNode
        let depth: Int
    }

    struct View {
        var filter: FolderFilter = .all
        var flatten = false
        var nameFilter = ""
    }

    static func matchesName(_ name: String, _ filter: String) -> Bool {
        cfBox("folder.matches_name", ["name": name, "filter": filter])?["matches"] as? Bool
            ?? filter.isEmpty
    }

    func rows(_ v: View) -> [Row] {
        let expanded = all.filter(\.expanded).map(\.id)
        let raw = cfBox("folder.rows", ["handle": handle, "filter": v.filter.rawValue,
                                        "nameFilter": v.nameFilter, "flatten": v.flatten,
                                        "expanded": expanded])?["rows"] as? [[String: Any]] ?? []
        return raw.compactMap { r in
            guard let id = r["id"] as? Int, let n = node(id) else { return nil }
            return Row(node: n, depth: r["depth"] as? Int ?? 0)
        }
    }

    func expandAll(_ open: Bool) { for n in all where n.isDir { n.expanded = open } }

    func settle() {
        let statuses: [[Any]] = all.map { [$0.id, $0.status.rawValue, $0.sameByMetadata] }
        applyStatuses(cfBox("folder.settle", ["handle": handle, "statuses": statuses])?["statuses"])
    }

    func applyStatuses(_ list: Any?) {
        for entry in (list as? [[Any]] ?? []) {
            guard entry.count >= 4, let id = entry[0] as? Int, let n = node(id) else { continue }
            n.status = FolderStatus(rawValue: entry[1] as? String ?? "same") ?? .same
            n.sameByMetadata = entry[2] as? Bool ?? false
            n.newer = FolderNewer(rawValue: entry[3] as? String ?? "none") ?? .none
        }
    }
}

enum FolderScan {
    static func isCaseInsensitive(_ path: String) -> Bool {
        let u = URL(fileURLWithPath: path)
        let v = try? u.resourceValues(forKeys: [.volumeSupportsCaseSensitiveNamesKey])
        return !(v?.volumeSupportsCaseSensitiveNames ?? false)
    }

    static func run(left: String, right: String, options o: FolderOptions,
                    progress: ((Int) -> Void)? = nil, cancelled: (() -> Bool)? = nil) -> FolderTree {
        let empty = FolderTree(left: left, right: right, handle: -1)
        empty.caseInsensitive = isCaseInsensitive(left)
        guard let start = cfBox("folder.scan_start",
                                ["left": left, "right": right, "options": o.json,
                                 "caseInsensitive": empty.caseInsensitive]),
              let session = start["session"] as? Int else { return empty }
        var last = 0
        while true {
            if cancelled?() == true { break }
            guard let step = cfBox("folder.scan_step", ["session": session, "maxDirs": 24]),
                  let done = step["done"] as? Bool else { break }
            let count = step["count"] as? Int ?? 0
            if count != last { last = count; progress?(count) }
            if done { break }
        }
        guard let snap = cfBox("folder.scan_finish", ["session": session]) else { return empty }
        return tree(from: snap, left: left, right: right)
    }

    static func tree(from snap: [String: Any], left: String, right: String) -> FolderTree {
        guard let handle = snap["handle"] as? Int else {
            return FolderTree(left: left, right: right, handle: -1)
        }
        let t = FolderTree(left: left, right: right, handle: handle)
        t.caseInsensitive = snap["caseInsensitive"] as? Bool ?? true
        t.truncated = snap["truncated"] as? Bool ?? false
        t.errors = snap["errors"] as? [String] ?? []
        let nodesJson = snap["nodes"] as? [[String: Any]] ?? []
        var byID: [Int: FolderNode] = [:]
        for j in nodesJson {
            guard let id = j["id"] as? Int else { continue }
            let n = FolderNode(id: id, key: j["key"] as? String ?? "",
                               rel: j["rel"] as? String ?? "", name: j["name"] as? String ?? "")
            n.left = (j["left"] as? [String: Any]).map(FolderSideInfo.init(json:))
            n.right = (j["right"] as? [String: Any]).map(FolderSideInfo.init(json:))
            n.status = FolderStatus(rawValue: j["status"] as? String ?? "same") ?? .same
            n.sameByMetadata = j["sameByMetadata"] as? Bool ?? false
            n.newer = FolderNewer(rawValue: j["newer"] as? String ?? "none") ?? .none
            n.depth = j["depth"] as? Int ?? 0
            byID[id] = n
            t.adopt(n)
        }
        for j in nodesJson {
            guard let id = j["id"] as? Int, let n = byID[id] else { continue }
            if let p = j["parent"] as? Int, let parent = byID[p] { n.parent = parent }
            n.children = (j["children"] as? [Int] ?? []).compactMap { byID[$0] }
        }
        t.roots = (snap["roots"] as? [Int] ?? []).compactMap { byID[$0] }
        return t
    }

    static func restat(_ n: FolderNode, tree: FolderTree, _ o: FolderOptions) {
        guard let box = cfBox("folder.restat", ["handle": tree.handle, "id": n.id,
                                                "options": o.json]) else { return }
        if let l = box["left"] as? [String: Any] { n.left = FolderSideInfo(json: l) }
        if let r = box["right"] as? [String: Any] { n.right = FolderSideInfo(json: r) }
        n.status = FolderStatus(rawValue: box["status"] as? String ?? "same") ?? .same
        n.sameByMetadata = box["sameByMetadata"] as? Bool ?? false
        n.newer = FolderNewer(rawValue: box["newer"] as? String ?? "none") ?? .none
    }
}

enum FolderContent {
    enum Answer { case same, different, unimportant, error }

    static func check(left: String, right: String, size: (Int64, Int64), imp: Importance) -> Answer {
        guard let box = cfBox("folder.content_check",
                              ["left": left, "right": right, "sizes": [size.0, size.1],
                               "importance": imp.json]) else { return .error }
        switch box["answer"] as? String {
        case "same": return .same
        case "unimportant": return .unimportant
        case "different": return .different
        default: return .error
        }
    }

    static func run(tree: FolderTree, nodes: [FolderNode], imp: Importance, queue: DispatchQueue,
                    cancelled: @escaping () -> Bool,
                    batch: @escaping ([(Int, Answer)]) -> Void, done: @escaping () -> Void) {
        struct Job { let id: Int; let l: String; let r: String; let size: (Int64, Int64) }
        let jobs = nodes.compactMap { n -> Job? in
            guard let l = n.left?.asFile, let r = n.right?.asFile else { return nil }
            return Job(id: n.id, l: tree.path(n, .left), r: tree.path(n, .right), size: (l.size, r.size))
        }
        queue.async {
            var pending: [(Int, Answer)] = []
            var last = Date()
            for j in jobs {
                if cancelled() { break }
                pending.append((j.id, check(left: j.l, right: j.r, size: j.size, imp: imp)))
                if Date().timeIntervalSince(last) > 0.1 { last = Date(); batch(pending); pending = [] }
            }
            if !pending.isEmpty { batch(pending) }
            done()
        }
    }

    static func apply(_ a: Answer, to n: FolderNode) {
        n.sameByMetadata = false
        switch a {
        case .same: n.status = .same
        case .different: n.status = .different
        case .unimportant: n.status = .unimportant
        case .error: n.status = .error
        }
    }

    static func ruleCandidates(_ tree: FolderTree) -> [FolderNode] {
        (cfBox("folder.rule_candidates", ["handle": tree.handle])?["ids"] as? [Int] ?? [])
            .compactMap(tree.node)
    }
}

enum SyncMode: String, CaseIterable {
    case updateRight, updateLeft, updateBoth, mirrorRight, mirrorLeft
    var title: String {
        switch self {
        case .updateRight: return "Update Right"
        case .updateLeft: return "Update Left"
        case .updateBoth: return "Update Both"
        case .mirrorRight: return "Mirror to Right"
        case .mirrorLeft: return "Mirror to Left"
        }
    }
    var explain: String {
        switch self {
        case .updateRight: return "Copy newer and left-only items to the right. Nothing is deleted."
        case .updateLeft: return "Copy newer and right-only items to the left. Nothing is deleted."
        case .updateBoth: return "Copy newer and orphan items each way. Nothing is deleted."
        case .mirrorRight: return "Make the right side the same as the left: copy every difference over, trash right-only items."
        case .mirrorLeft: return "Make the left side the same as the right: copy every difference over, trash left-only items."
        }
    }
}

struct SyncPlan {
    struct Copy: Equatable { let src: String; let dst: String; let to: CompareSide; let rel: String; let replaces: Bool }
    var copies: [Copy] = []
    var trash: [(path: String, side: CompareSide, rel: String)] = []
    var skipped: [String] = []
    var isEmpty: Bool { copies.isEmpty && trash.isEmpty }

    static func make(_ tree: FolderTree, _ mode: SyncMode, nameFilter: String = "") -> SyncPlan {
        var p = SyncPlan()
        guard let box = cfBox("folder.sync_plan", ["handle": tree.handle, "mode": mode.rawValue,
                                                   "nameFilter": nameFilter]) else { return p }
        p.copies = (box["copies"] as? [[String: Any]] ?? []).compactMap { c in
            guard let src = c["src"] as? String, let dst = c["dst"] as? String,
                  let to = c["to"] as? String, let side = CompareSide(rawValue: to) else { return nil }
            return Copy(src: src, dst: dst, to: side, rel: c["rel"] as? String ?? "",
                        replaces: c["replaces"] as? Bool ?? false)
        }
        p.trash = (box["trash"] as? [[String: Any]] ?? []).compactMap { t in
            guard let path = t["path"] as? String, let to = t["side"] as? String,
                  let side = CompareSide(rawValue: to) else { return nil }
            return (path: path, side: side, rel: t["rel"] as? String ?? "")
        }
        p.skipped = box["skipped"] as? [String] ?? []
        return p
    }
}
