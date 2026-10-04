import Foundation

// Folder Compare's engine (PRD-compare.md §7.2.3). Foundation only —
// bin/run-tests.sh compare drives it on real folders.
//
//   FolderScan.run     both trees walked off main → a `FolderTree` of nodes
//                      paired by relative path (case per volume, NFC / NFD
//                      equal), quick test (size + time ± tolerance) → status
//   FolderTree.pending file pairs the quick test could not settle: the
//                      caller checks them on a background queue
//                      (`FolderContent.check`) and applies the answers
//   FolderTree.rows    the display rows: expansion, filter, flatten, name
//                      filter
//
// Status colors (drawn by the view): same · different (+ newer side) ·
// unimportant (text equal after the importance rules: blue) · left / right
// orphan · unknown (content not checked yet) · error.

enum FolderStatus: String {
    case same, different, unimportant, leftOnly, rightOnly, unknown, error
    var isOrphan: Bool { self == .leftOnly || self == .rightOnly }
    // counts as "a difference" for the Differences filter / Ctrl+N
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

// one side of a pair
struct FolderSideInfo {
    var name: String            // the on-disk name (case as stored)
    var isDir: Bool
    var isLink: Bool
    var link: String?           // a symlink's target (compared as text, never walked into)
    var size: Int64
    var mtime: Double
    // a symlink to a regular file: that file's size + time. A link facing a
    // regular file on the other side compares by its target (`git difftool
    // -d` links the work tree's files into its right-hand folder)
    var targetSize: Int64? = nil
    var targetMtime: Double = 0

    // what the quick test compares: the file itself, or a link's target file
    var asFile: (size: Int64, mtime: Double)? {
        if isLink { return targetSize.map { ($0, targetMtime) } }
        return isDir ? nil : (size, mtime)
    }

    // one entry, lstat'ed (links described, never followed for the tree)
    static func read(_ path: String, name: String) -> FolderSideInfo? {
        var st = stat()
        guard lstat(path, &st) == 0 else { return nil }
        let mode = st.st_mode & S_IFMT
        let isLink = mode == S_IFLNK
        let isDir = mode == S_IFDIR
        let mtime = Double(st.st_mtimespec.tv_sec) + Double(st.st_mtimespec.tv_nsec) / 1e9
        var info = FolderSideInfo(name: name, isDir: isDir, isLink: isLink,
                                  link: isLink ? try? FileManager.default.destinationOfSymbolicLink(atPath: path) : nil,
                                  size: isDir ? 0 : Int64(st.st_size), mtime: mtime)
        if isLink {
            var t = stat()
            if stat(path, &t) == 0, t.st_mode & S_IFMT == S_IFREG {
                info.targetSize = Int64(t.st_size)
                info.targetMtime = Double(t.st_mtimespec.tv_sec) + Double(t.st_mtimespec.tv_nsec) / 1e9
            }
        }
        return info
    }
}

final class FolderNode {
    let id: Int
    let key: String             // the pairing key: relative path (folded)
    let rel: String             // the relative path shown (the left name wins)
    var left: FolderSideInfo?
    var right: FolderSideInfo?
    var status: FolderStatus = .same
    var newer: FolderNewer = .none
    var children: [FolderNode] = []
    weak var parent: FolderNode?
    var expanded = false
    var depth = 0
    // folders: how many descendants carry each flag (the folder's own color)
    var diffBelow = 0, importantBelow = 0, unknownBelow = 0
    init(id: Int, key: String, rel: String) { self.id = id; self.key = key; self.rel = rel }

    var name: String { (left?.name ?? right?.name) ?? (rel as NSString).lastPathComponent }
    var isDir: Bool { (left?.isDir ?? false) || (right?.isDir ?? false) }
    // a pair whose two sides are the same KIND (both folders / both files)
    var kindMismatch: Bool { if let l = left, let r = right { return l.isDir != r.isDir } else { return false } }
}

struct FolderOptions {
    var timeTolerance: Double = 2           // seconds two mtimes may differ and still be "same"
    var content = "auto"                    // auto | always | never
    var hidden = true                       // dot files listed
    var exclude: [String] = []              // names / globs never scanned
    var ignored: ((_ path: String, _ isDir: Bool) -> Bool)?   // gitignore rules (path = absolute)
    var importance = Importance()           // the Text Compare rules (unimportant)
}

// a finished scan
final class FolderTree {
    let leftRoot: String
    let rightRoot: String
    var roots: [FolderNode] = []
    private(set) var all: [FolderNode] = []
    var caseInsensitive = true
    var truncated = false
    var errors: [String] = []

    init(left: String, right: String) { leftRoot = left; rightRoot = right }

    func adopt(_ n: FolderNode) { all.append(n) }
    func node(_ id: Int) -> FolderNode? { id >= 0 && id < all.count ? all[id] : nil }

    func path(_ n: FolderNode, _ side: CompareSide) -> String {
        let root = side == .left ? leftRoot : rightRoot
        // the name as stored on THAT side (a case-only difference is a real path)
        var parts: [String] = []
        var cur: FolderNode? = n
        while let c = cur {
            parts.append((side == .left ? c.left?.name : c.right?.name) ?? c.name)
            cur = c.parent
        }
        return parts.reversed().reduce(root) { ($0 as NSString).appendingPathComponent($1) }
    }

    // file pairs the quick test left open (status unknown)
    var pending: [FolderNode] { all.filter { $0.status == .unknown && !$0.isDir } }

    // MARK: summary

    struct Counts { var different = 0, unimportant = 0, leftOnly = 0, rightOnly = 0, same = 0, unknown = 0, error = 0 }
    // files only (a folder is its content)
    func counts() -> Counts {
        var c = Counts()
        for n in all where !n.isDir || n.kindMismatch {
            switch n.status {
            case .different: c.different += 1
            case .unimportant: c.unimportant += 1
            case .leftOnly: c.leftOnly += 1
            case .rightOnly: c.rightOnly += 1
            case .same: c.same += 1
            case .unknown: c.unknown += 1
            case .error: c.error += 1
            }
        }
        return c
    }

    // MARK: derived state

    // a folder's color = what is below it. Run after the scan and after every
    // batch of content answers.
    func settle() {
        func walk(_ n: FolderNode) {
            for c in n.children { walk(c) }
            guard n.isDir, !n.kindMismatch else { return }
            func isFolder(_ c: FolderNode) -> Bool { c.isDir && !c.kindMismatch }
            n.diffBelow = n.children.reduce(0) { $0 + (isFolder($1) ? $1.diffBelow : ($1.status.isDiff ? 1 : 0)) }
            n.unknownBelow = n.children.reduce(0) { $0 + (isFolder($1) ? $1.unknownBelow : ($1.status == .unknown ? 1 : 0)) }
            n.importantBelow = n.children.reduce(0) { sum, c in
                sum + (isFolder(c) ? c.importantBelow : (c.status.isDiff && c.status != .unimportant && c.status != .unknown ? 1 : 0))
            }
            if n.left != nil && n.right == nil { n.status = .leftOnly; return }
            if n.right != nil && n.left == nil { n.status = .rightOnly; return }
            if n.importantBelow > 0 { n.status = .different }
            else if n.unknownBelow > 0 { n.status = .unknown }
            else if n.diffBelow > 0 { n.status = .unimportant }
            else { n.status = .same }
            n.newer = .none
        }
        roots.forEach(walk)
    }

    // MARK: rows

    struct Row {
        let node: FolderNode
        let depth: Int
    }

    struct View {
        var filter: FolderFilter = .all
        var flatten = false
        var nameFilter = ""             // "*.swift, !*.o"
    }

    // does a FILE node pass the status filter
    static func passes(_ n: FolderNode, _ f: FolderFilter) -> Bool {
        switch f {
        case .all: return true
        case .diffs: return n.status.isDiff
        case .same: return n.status == .same
        case .orphans: return n.status.isOrphan
        case .leftNewer: return n.newer == .left
        case .rightNewer: return n.newer == .right
        }
    }

    static func matchesName(_ name: String, _ filter: String) -> Bool {
        let parts = filter.split(whereSeparator: { $0 == "," || $0 == " " }).map(String.init).filter { !$0.isEmpty }
        if parts.isEmpty { return true }
        let inc = parts.filter { !$0.hasPrefix("!") }, exc = parts.filter { $0.hasPrefix("!") }.map { String($0.dropFirst()) }
        func hit(_ g: String) -> Bool {
            let pat = g.contains("*") || g.contains("?") || g.contains("[") ? g : "*\(g)*"
            return fnmatch(pat, name, FNM_CASEFOLD) == 0
        }
        if exc.contains(where: hit) { return false }
        return inc.isEmpty || inc.contains(where: hit)
    }

    // the visible rows. A folder shows when something under it passes (All
    // and a folder-only status: always); `expanded` decides its children.
    func rows(_ v: View) -> [Row] {
        var out: [Row] = []
        let narrowing = v.filter != .all || !v.nameFilter.isEmpty
        if v.flatten {
            for n in all where !n.isDir || n.kindMismatch {
                if Self.passes(n, v.filter), Self.matchesName(n.name, v.nameFilter) { out.append(Row(node: n, depth: 0)) }
            }
            return out
        }
        // does this node (or anything below it) pass?
        func shows(_ n: FolderNode) -> Bool {
            if !n.isDir || n.kindMismatch { return Self.passes(n, v.filter) && Self.matchesName(n.name, v.nameFilter) }
            if !narrowing { return true }
            return n.children.contains(where: shows)
        }
        func walk(_ n: FolderNode, _ depth: Int) {
            guard shows(n) else { return }
            out.append(Row(node: n, depth: depth))
            // a narrowed list opens its folders: you filtered to see the files
            if n.isDir, !n.kindMismatch, n.expanded || narrowing {
                for c in n.children { walk(c, depth + 1) }
            }
        }
        roots.forEach { walk($0, 0) }
        return out
    }

    func expandAll(_ open: Bool) { for n in all where n.isDir { n.expanded = open } }
}

// MARK: - scan

enum FolderScan {
    struct Entry {
        var name: String
        var info: FolderSideInfo
    }

    static func isCaseInsensitive(_ path: String) -> Bool {
        let u = URL(fileURLWithPath: path)
        let v = try? u.resourceValues(forKeys: [.volumeSupportsCaseSensitiveNamesKey])
        return !(v?.volumeSupportsCaseSensitiveNames ?? false)
    }

    // the key two names pair on
    static func fold(_ name: String, _ ci: Bool) -> String {
        let n = name.precomposedStringWithCanonicalMapping
        return ci ? n.lowercased() : n
    }

    static func list(_ dir: String, hidden: Bool) -> [Entry]? {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: dir) else { return nil }
        var out: [Entry] = []
        out.reserveCapacity(names.count)
        for name in names {
            if !hidden && name.hasPrefix(".") { continue }
            guard let info = FolderSideInfo.read((dir as NSString).appendingPathComponent(name), name: name) else { continue }
            out.append(Entry(name: name, info: info))
        }
        return out
    }

    // does an `exclude` entry hit this name
    static func excluded(_ name: String, _ patterns: [String]) -> Bool {
        patterns.contains { p in
            p.contains("*") || p.contains("?") || p.contains("[") ? fnmatch(p, name, 0) == 0 : p == name
        }
    }

    // both trees, paired. `progress(n)` ≈ every 100 ms with the node count;
    // `cancelled()` is polled per folder.
    static func run(left: String, right: String, options o: FolderOptions,
                    progress: ((Int) -> Void)? = nil, cancelled: (() -> Bool)? = nil) -> FolderTree {
        let tree = FolderTree(left: left, right: right)
        tree.caseInsensitive = isCaseInsensitive(left)
        var last = Date()
        func tick() {
            if let p = progress, Date().timeIntervalSince(last) > 0.1 { last = Date(); p(tree.all.count) }
        }

        // `lp` / `rp`: the folder on each side (nil = that side has none here)
        func pair(_ lp: String?, _ rp: String?, rel: String, parent: FolderNode?, depth: Int) -> [FolderNode] {
            if cancelled?() == true { return [] }
            var le = lp.flatMap { list($0, hidden: o.hidden) }, re = rp.flatMap { list($0, hidden: o.hidden) }
            if lp != nil && le == nil { tree.errors.append(lp!); le = [] }
            if rp != nil && re == nil { tree.errors.append(rp!); re = [] }
            func keep(_ e: Entry, in dir: String?) -> Bool {
                if excluded(e.name, o.exclude) { return false }
                if let ig = o.ignored, let d = dir, ig((d as NSString).appendingPathComponent(e.name), e.info.isDir) { return false }
                return true
            }
            let l = (le ?? []).filter { keep($0, in: lp) }, r = (re ?? []).filter { keep($0, in: rp) }
            var byKey: [String: (l: Entry?, r: Entry?)] = [:]
            var order: [String] = []
            for e in l {
                let k = fold(e.name, tree.caseInsensitive)
                if byKey[k] == nil { order.append(k) }
                byKey[k, default: (nil, nil)].l = e
            }
            for e in r {
                let k = fold(e.name, tree.caseInsensitive)
                if byKey[k] == nil { order.append(k) }
                byKey[k, default: (nil, nil)].r = e
            }
            // folders first, then by name (Finder-like, case-insensitive)
            let sorted = order.sorted { a, b in
                let ea = byKey[a]!, eb = byKey[b]!
                let da = (ea.l?.info.isDir ?? ea.r?.info.isDir) ?? false, db = (eb.l?.info.isDir ?? eb.r?.info.isDir) ?? false
                if da != db { return da }
                let na = ea.l?.name ?? ea.r?.name ?? "", nb = eb.l?.name ?? eb.r?.name ?? ""
                return na.localizedStandardCompare(nb) == .orderedAscending
            }
            var nodes: [FolderNode] = []
            for k in sorted {
                if tree.all.count >= 400_000 { tree.truncated = true; break }
                let e = byKey[k]!
                let name = e.l?.name ?? e.r?.name ?? k
                let n = FolderNode(id: tree.all.count, key: rel.isEmpty ? k : rel + "/" + k,
                                   rel: rel.isEmpty ? name : rel + "/" + name)
                tree.adopt(n)
                n.left = e.l?.info
                n.right = e.r?.info
                n.parent = parent
                n.depth = depth
                nodes.append(n)
                tick()
                classify(n, left: lp.map { ($0 as NSString).appendingPathComponent(e.l?.name ?? "") },
                         right: rp.map { ($0 as NSString).appendingPathComponent(e.r?.name ?? "") }, o)
                if n.isDir && !n.kindMismatch {
                    let ld = n.left != nil ? lp.map { ($0 as NSString).appendingPathComponent(n.left!.name) } : nil
                    let rd = n.right != nil ? rp.map { ($0 as NSString).appendingPathComponent(n.right!.name) } : nil
                    n.children = pair(ld, rd, rel: n.key, parent: n, depth: depth + 1)
                }
            }
            return nodes
        }

        tree.roots = pair(left, right, rel: "", parent: nil, depth: 0)
        tree.settle()
        return tree
    }

    // the quick test for one node (files; a folder's color comes from `settle`)
    static func classify(_ n: FolderNode, left lp: String?, right rp: String?, _ o: FolderOptions) {
        switch (n.left, n.right) {
        case (.some, nil): n.status = .leftOnly; return
        case (nil, .some): n.status = .rightOnly; return
        case (nil, nil): n.status = .error; return
        default: break
        }
        guard let l = n.left, let r = n.right else { return }
        if l.isDir != r.isDir { n.status = .different; return }
        if l.isDir { return }
        if l.isLink && r.isLink {
            n.status = l.link == r.link ? .same : .different
            return
        }
        // a link facing a regular file: its target file is compared
        guard let lf = l.asFile, let rf = r.asFile else { n.status = .different; return }
        n.newer = lf.mtime > rf.mtime + o.timeTolerance ? .left : rf.mtime > lf.mtime + o.timeTolerance ? .right : .none
        if lf.size != rf.size {
            n.status = .different
            return
        }
        let timesMatch = abs(lf.mtime - rf.mtime) <= o.timeTolerance
        switch o.content {
        case "always": n.status = .unknown
        case "never": n.status = timesMatch ? .same : .different
        default: n.status = timesMatch ? .same : .unknown        // auto: same size, different time
        }
    }

    // a node's status after an operation touched it: stat both sides again
    // and run the quick test (callers then check content if it is unknown)
    static func restat(_ n: FolderNode, tree: FolderTree, _ o: FolderOptions) {
        let lp = tree.path(n, .left), rp = tree.path(n, .right)
        n.left = FolderSideInfo.read(lp, name: (lp as NSString).lastPathComponent)
        n.right = FolderSideInfo.read(rp, name: (rp as NSString).lastPathComponent)
        n.newer = .none
        classify(n, left: lp, right: rp, o)
    }
}

// MARK: - content

enum FolderContent {
    enum Answer { case same, different, unimportant, error }

    // byte compare in 1 MB chunks, early exit
    static func sameBytes(_ a: String, _ b: String) -> Bool? {
        guard let fa = FileHandle(forReadingAtPath: a), let fb = FileHandle(forReadingAtPath: b) else { return nil }
        defer { try? fa.close(); try? fb.close() }
        let chunk = 1 << 20
        while true {
            let da = (try? fa.read(upToCount: chunk)) ?? Data()
            let db = (try? fb.read(upToCount: chunk)) ?? Data()
            if da != db { return false }
            if da.isEmpty { return true }
        }
    }

    // text files that differ get one pass of the Text Compare normalizer:
    // equal after the importance rules = unimportant (blue)
    static func textEqualUnderRules(_ a: String, _ b: String, _ imp: Importance, limit: Int = 4 << 20) -> Bool {
        guard let da = FileManager.default.contents(atPath: a), let db = FileManager.default.contents(atPath: b),
              da.count <= limit, db.count <= limit,
              !TextSide.isBinary(da), !TextSide.isBinary(db),
              let ta = TextSide.decode(da), let tb = TextSide.decode(db) else { return false }
        if ta.lines.count != tb.lines.count && !imp.blankLines { return false }
        let ka = zip(ta.lines, ta.eols).map { imp.key($0, $1) }.filter { !(imp.blankLines && $0.isEmpty) }
        let kb = zip(tb.lines, tb.eols).map { imp.key($0, $1) }.filter { !(imp.blankLines && $0.isEmpty) }
        return ka == kb
    }

    static func check(left: String, right: String, size: (Int64, Int64), imp: Importance) -> Answer {
        if size.0 == size.1 {
            switch sameBytes(left, right) {
            case .some(true): return .same
            case .none: return .error
            default: break
            }
        }
        return textEqualUnderRules(left, right, imp) ? .unimportant : .different
    }

    // run the open file pairs on `queue`; `batch` gets (node id, answer) in
    // groups (≈ every 100 ms) on the QUEUE's thread; `done` when finished.
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

    // apply one answer to its node
    static func apply(_ a: Answer, to n: FolderNode) {
        switch a {
        case .same: n.status = .same
        case .different: n.status = .different
        case .unimportant: n.status = .unimportant
        case .error: n.status = .error
        }
    }

    // different-size pairs (and "never"-checked different ones) can still be
    // equal under the rules: nodes worth the text pass
    static func ruleCandidates(_ tree: FolderTree) -> [FolderNode] {
        tree.all.filter { n in
            guard n.status == .different, let l = n.left, let r = n.right, !(l.isLink && r.isLink),
                  let lf = l.asFile, let rf = r.asFile else { return false }
            return lf.size <= 4 << 20 && rf.size <= 4 << 20
        }
    }
}

// MARK: - Synchronize (phase 3)

// Beyond Compare's Synchronize: what to copy / trash so one side (or both)
// catches up. Always shown in a preview sheet before it runs; deletes go to
// the Trash. Built from a finished tree (statuses as shown).
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
    var skipped: [String] = []          // rel paths that differ with neither side newer (Update only)
    var isEmpty: Bool { copies.isEmpty && trash.isEmpty }

    static func make(_ tree: FolderTree, _ mode: SyncMode, nameFilter: String = "") -> SyncPlan {
        var p = SyncPlan()
        let toRight: Bool = [.updateRight, .updateBoth, .mirrorRight].contains(mode)
        let toLeft: Bool = [.updateLeft, .updateBoth, .mirrorLeft].contains(mode)
        let mirror: CompareSide? = mode == .mirrorRight ? .right : mode == .mirrorLeft ? .left : nil
        func copy(_ n: FolderNode, to side: CompareSide) {
            let other = side == .left ? n.left : n.right
            p.copies.append(Copy(src: tree.path(n, side.other), dst: tree.path(n, side), to: side, rel: n.rel, replaces: other != nil))
        }
        func passes(_ n: FolderNode) -> Bool { nameFilter.isEmpty || FolderTree.matchesName(n.name, nameFilter) }
        func visit(_ n: FolderNode) {
            let folder = n.isDir && !n.kindMismatch
            // a pair of folders: what is inside decides
            if folder && n.left != nil && n.right != nil { n.children.forEach(visit); return }
            // an orphan folder goes whole, unless a name filter picks files in it
            if folder && !nameFilter.isEmpty { n.children.forEach(visit); return }
            if !folder && !passes(n) { return }
            switch n.status {
            case .leftOnly:
                if mirror == .left { p.trash.append((tree.path(n, .left), .left, n.rel)) } else if toRight { copy(n, to: .right) }
            case .rightOnly:
                if mirror == .right { p.trash.append((tree.path(n, .right), .right, n.rel)) } else if toLeft { copy(n, to: .left) }
            case .different, .unknown:
                if let m = mirror { copy(n, to: m); return }
                switch n.newer {
                case .left where toRight: copy(n, to: .right)
                case .right where toLeft: copy(n, to: .left)
                case .none: p.skipped.append(n.rel)
                default: break
                }
            case .same, .unimportant, .error: break
            }
        }
        tree.roots.forEach(visit)
        return p
    }
}
