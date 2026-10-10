import AppKit
import Foundation
import Darwin
import AVFoundation
import Speech

final class ClosureMenuItem: NSMenuItem {
    private let handler: () -> Void
    init(_ title: String, _ handler: @escaping () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError("init(coder:) not supported") }
    @objc private func fire() { handler() }
}

func menuItem(_ title: String, state: Bool? = nil, enabled: Bool = true,
              _ action: @escaping () -> Void) -> NSMenuItem {
    let item = ClosureMenuItem(title, action)
    if let state { item.state = state ? .on : .off }
    item.isEnabled = enabled
    return item
}

final class ClosureTarget: NSObject {
    private let action: () -> Void
    init(action: @escaping () -> Void) { self.action = action }
    @objc func run() { action() }
}

private let bundleParentDir: String = {
    let u = URL(fileURLWithPath: CommandLine.arguments[0]).absoluteURL.resolvingSymlinksInPath()
    let p = u.path
    if let r = p.range(of: "/Contents/MacOS/") {
        var d = String(p[..<r.lowerBound])
        if d.hasSuffix(".app") { d = (d as NSString).deletingLastPathComponent }
        return d
    }
    return u.deletingLastPathComponent().path
}()
let appBundlePath: String? = {
    let p = URL(fileURLWithPath: CommandLine.arguments[0]).absoluteURL.resolvingSymlinksInPath().path
    guard let r = p.range(of: "/Contents/MacOS/") else { return nil }
    return String(p[..<r.lowerBound])
}()
let isRepoBuild: Bool = {
    let fm = FileManager.default
    return fm.fileExists(atPath: bundleParentDir + "/commands.toml")
        && fm.fileExists(atPath: bundleParentDir + "/bin/build-app.sh")
}()
let homeDir: String = {
    if let h = ProcessInfo.processInfo.environment["WS_HOME"], !h.isEmpty { return h }
    return NSHomeDirectory() + "/.config/kitchen-sink"
}()
let assetDir: String = {
    if isRepoBuild { return bundleParentDir }
    if let b = appBundlePath { return b + "/Contents/Resources" }
    return bundleParentDir
}()
let userDir: String = isRepoBuild ? bundleParentDir : homeDir
let pythonHelper = PythonHelper.shared

let commandsConfName = "commands.toml"

struct AppSettings {
    var sharedWindow = true
    var preload = true
    var sharedWidth: CGFloat = 1100
    var sharedHeight: CGFloat = 640
    var marginTop: CGFloat = 0
    var marginTopBuiltin: CGFloat = 0
    var marginBottom: CGFloat = 0
    var switcherWidth: CGFloat = 760
    var paletteFirst: [String] = ["filefast", "paths", "prettyprint"]
    var shell = "/opt/homebrew/bin/bash"
    var shellArgs: [String] = ["--login", "-i"]
    var terminalFont = "Hack Nerd Font"
    var terminalFontSize: CGFloat = 13
    var fontInstallCasks: [(label: String, cask: String, type: String)] = [
        ("JetBrains Mono Nerd Font", "font-jetbrains-mono-nerd-font", "nerd"),
        ("Fira Code Nerd Font", "font-fira-code-nerd-font", "nerd"),
        ("Iosevka Term Nerd Font", "font-iosevka-term-nerd-font", "nerd"),
        ("IBM Plex Mono", "font-ibm-plex-mono", "mono"),
        ("Cascadia Code", "font-cascadia-code", "mono"),
        ("Inter", "font-inter", "sans"),
        ("Source Serif 4", "font-source-serif-4", "serif"),
    ]
    var aerospaceCLI = ["/opt/homebrew/bin/aerospace",
                        "/usr/local/bin/aerospace", "aerospace"]
    var appDirs = ["/Applications", "/Applications/Utilities",
                   "/System/Applications", "/System/Applications/Utilities",
                   "/System/Library/CoreServices",
                   NSHomeDirectory() + "/Applications"]
    var jiraIconName = "jira_icon.png"
    var confluenceIconName = "confluence_icon.png"
    var aiIconName = ""
    var appIconName = "app_icon.png"
    var notesIconName = "notes_icon.png"
    var filesIconName = ""
    var notesSocketName = "ws-notes.sock"
    var focusFileName = "kitchen-sink-focus"
    var focusBridgeName = "ws-aerospace-focus"
    var switcherWindowName = "kitchen-sink"
    var detailWindowName = "jira-detail"
    var aerospaceSocketPath = "/tmp/bobko.aerospace-\(NSUserName()).sock"
    var crashLogPath = NSString(string: "~/.cache/ws-crash.log").expandingTildeInPath
    var aeroDebugFlag = NSString(string: "~/.cache/aero-debug").expandingTildeInPath
    var aeroLog = NSString(string: "~/.cache/ws-aero.log").expandingTildeInPath
    var voiceLocale = "en-US"
    var screenshotApps = ["org.flameshot", "pl.maketheweb.cleanshotx",
                          "cc.ffitch.shottr", "com.skitch.skitch"]
    var hideOnFocusLoss = true
    var focusLossDelay = 0.3
    var escClose = 0
    var copyToast = "Copied {} to clipboard"
    var terminalApp = ""
    var commandsConfPath: String { userDir + "/" + commandsConfName }
    var focusFilePath: String { popupTmpDir() + focusFileName }
    var jiraIconPath: String { assetDir + "/" + jiraIconName }
    var confluenceIconPath: String { assetDir + "/" + confluenceIconName }
    var aiIconPath: String { aiIconName.isEmpty ? "" : (aiIconName.hasPrefix("/") ? aiIconName : assetDir + "/" + aiIconName) }
    var notesIconPath: String { assetDir + "/" + notesIconName }
    var appIconPath: String { assetDir + "/" + appIconName }
    var filesIconPath: String {
        filesIconName.isEmpty || filesIconName.hasPrefix("/") ? filesIconName : assetDir + "/" + filesIconName
    }
}
var settings = AppSettings()

let presetMinOpacity: CGFloat = 0.6
let ipcSocketTimeout = 1.0
let ipcFallbackTimeout = 1.5
let serverRecvTimeout = 2.0
let noteWatchInterval = 1.0
let listWatchInterval = 1.5
let defaultNoteSize = CGSize(width: 640, height: 440)
let defaultListSize = CGSize(width: 520, height: 520)
let defaultOutputSize = CGSize(width: 780, height: 560)
let defaultDetailSize = CGSize(width: 820, height: 640)

private let crashLogFD: Int32 = {
    if let dir = (settings.crashLogPath as NSString).deletingLastPathComponent as String? {
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    }
    return Darwin.open(settings.crashLogPath, O_WRONLY | O_CREAT | O_APPEND, 0o644)
}()

let crashHandler: @convention(c) (Int32) -> Void = { sig in
    let fd = crashLogFD
    guard fd >= 0 else { return }
    let header = "\n===== ws crash \(Date()) signal=\(sig) ====="
    header.withCString { _ = Darwin.write(fd, $0, strlen($0)) }
    var frames = [UnsafeMutableRawPointer?](repeating: nil, count: 128)
    let n = backtrace(&frames, 128)
    backtrace_symbols_fd(&frames, n, fd)
    _ = Darwin.write(fd, "\n", 1)
    signal(sig, SIG_DFL)
    raise(sig)
}

func installCrashHandler() {
    for sig in [SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGSEGV, SIGFPE] {
        signal(sig, crashHandler)
    }
}

func jiraWindowColors() -> PopupColors {
    let v = { (k: String) in hexColor(jiraConfigValue(k)) }
    var c = PopupColors(background: v("background-color").map { ($0.usingColorSpace(.sRGB) ?? $0).withAlphaComponent(1) } ?? BAR,
                        border: BORDER, text: v("text-color") ?? TEXT, dim: v("dim-color") ?? DIM,
                        highlight: v("highlight-color") ?? GROUP_BG, accent: v("accent-color") ?? ACCENT,
                        palette: parsePalette(jiraConfigValue("palette")) ?? THEME_PALETTE)
    if v("accent-color") != nil || THEME["border"] == nil { c.border = c.outline }
    return c
}
var jiraHeaderColor: NSColor { hexColor(jiraConfigValue("header-color")) ?? headerBlueSilver }

func jiraCellStyle(_ field: String, _ text: String) -> PopupCellStyle? {
    let v = text.lowercased(), w = JiraWords.current
    switch field.lowercased() {
    case "key", "updated", "created", "duedate", "releasedate", "project", "releaselabel", "release":
        return PopupCellStyle(.dim)
    case "status", "statuscategory":
        if w.matches(v, w.cancelled) { return PopupCellStyle(.dim, mark: .hollow, quietsRow: true) }
        if w.matches(v, w.done) { return PopupCellStyle(.success, mark: .filled, quietsRow: true) }
        if w.matches(v, w.blocked) { return PopupCellStyle(.danger, mark: .filled) }
        if w.matches(v, w.active) { return PopupCellStyle(.info, mark: .half) }
        if w.matches(v, w.waiting) { return PopupCellStyle(.warning, mark: .hollow) }
        return PopupCellStyle(.dim, mark: .hollow)
    case "priority":
        if w.matches(v, w.urgent) { return PopupCellStyle(.danger, tinted: true, bold: true) }
        return PopupCellStyle(.dim)
    case "releasestatus":
        if v.contains("unreleased") { return PopupCellStyle(.warning, mark: .hollow) }
        if v.contains("released") { return PopupCellStyle(.success, mark: .filled) }
        return nil
    default: return nil
    }
}

struct JiraWords {
    let cancelled, done, blocked, active, waiting, new, urgent: [String]
    static let defaults: [String: String] = [
        "status-cancelled-words": "cancel, won't, wont, reject, duplicate",
        "status-done-words": "done, closed, resolved, released, complete, fixed, shipped",
        "status-blocked-words": "block, fail, impediment",
        "status-active-words": "progress, review, test, qa, develop, doing, verif",
        "status-waiting-words": "hold, wait, pending, paused",
        "status-new-words": "backlog, open, to do, todo, new, selected, triage, funnel",
        "priority-urgent-words": "highest, blocker, critical, urgent, p0, p1",
    ]
    private static var cache: JiraWords?
    private static let lock = NSLock()
    static var current: JiraWords {
        lock.lock(); defer { lock.unlock() }
        if let c = cache { return c }
        let c = JiraWords { jiraConfigValue($0) }
        cache = c
        return c
    }
    static func invalidate() { lock.lock(); cache = nil; lock.unlock() }

    init(_ value: (String) -> String?) {
        func list(_ key: String) -> [String] {
            (value(key) ?? Self.defaults[key] ?? "").split(separator: ",")
                .map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty }
        }
        cancelled = list("status-cancelled-words"); done = list("status-done-words")
        blocked = list("status-blocked-words"); active = list("status-active-words")
        waiting = list("status-waiting-words"); new = list("status-new-words")
        urgent = list("priority-urgent-words")
    }
    func matches(_ lowered: String, _ words: [String]) -> Bool { words.contains(where: lowered.contains) }
}

func parsePalette(_ v: String?) -> PopupPalette? {
    guard let v, !v.isEmpty else { return nil }
    let cs = v.split(separator: ",").compactMap { hexColor($0.trimmingCharacters(in: .whitespaces)) }
    return PopupPalette(cs)
}
func paletteString(_ p: PopupPalette) -> String {
    p.all.map { c in
        let cc = c.usingColorSpace(.sRGB) ?? c
        return String(format: "%02X%02X%02X", Int(round(cc.redComponent * 255)),
                      Int(round(cc.greenComponent * 255)), Int(round(cc.blueComponent * 255)))
    }.joined(separator: ", ")
}

func parseTheme() -> [String: NSColor] {
    var out: [String: NSColor] = [:]
    guard let content = readConfigText() else { return out }
    for e in configSectionEntries(configLines(content), "theme") {
        if let c = hexColor(e.value) { out[e.key.lowercased()] = c }
    }
    return out
}

let THEME = parseTheme()
let BAR = THEME["background"] ?? NSColor(srgbRed: 0x24/255, green: 0x27/255, blue: 0x3A/255, alpha: 1)
let GROUP_BG = THEME["highlight"] ?? NSColor(srgbRed: 0x3F/255, green: 0x4A/255, blue: 0x5A/255, alpha: 1)
let TEXT = THEME["text"] ?? NSColor(srgbRed: 0xCA/255, green: 0xD3/255, blue: 0xF5/255, alpha: 1)
let DIM = THEME["dim"] ?? NSColor(srgbRed: 0x93/255, green: 0x9A/255, blue: 0xB7/255, alpha: 1)
let BORDER = THEME["border"] ?? NSColor(srgbRed: 0xC6/255, green: 0xA0/255, blue: 0xF6/255, alpha: 1)
let ACCENT = THEME["accent"] ?? NSColor(srgbRed: 85/255, green: 104/255, blue: 130/255, alpha: 1)
let THEME_HEADER = THEME["header"]
let THEME_BROWSER = THEME["browser"]
let THEME_TERMINAL = THEME["terminal"]

let headerBlueSilver = THEME_HEADER ?? NSColor(srgbRed: 0.27, green: 0.31, blue: 0.36, alpha: 1)

let THEME_PALETTE: PopupPalette = {
    let d = PopupPalette()
    return PopupPalette(accent2: THEME["accent2"] ?? d.accent2, success: THEME["success"] ?? d.success,
                        warning: THEME["warning"] ?? d.warning, danger: THEME["danger"] ?? d.danger,
                        info: THEME["info"] ?? d.info)
}()

func windowColors(_ cmd: CommandSpec? = nil) -> PopupColors {
    var c = PopupColors(background: BAR, border: BORDER,
                        text: cmd?.textColor ?? TEXT, dim: cmd?.dimColor ?? DIM,
                        highlight: cmd?.highlightColor ?? GROUP_BG,
                        accent: cmd?.accentColor ?? ACCENT,
                        palette: cmd?.palette ?? THEME_PALETTE)
    if let bg = cmd?.backgroundColor {
        c.background = (bg.usingColorSpace(.sRGB) ?? bg).withAlphaComponent(1)
    }
    if cmd?.accentColor != nil || THEME["border"] == nil { c.border = c.outline }
    return c
}

func applyWindowTheme(_ cfg: inout PopupConfig, _ cmd: CommandSpec) {
    cfg.headerColor = cmd.headerColor ?? headerBlueSilver
    cfg.colors = windowColors(cmd)
    if let ta = cmd.tintAlpha { cfg.tintAlpha = ta }
    if let bg = cmd.backgroundColor { cfg.tintAlpha = (bg.usingColorSpace(.sRGB) ?? bg).alphaComponent }
    cfg.fontName = cmd.font
}

@discardableResult
func sendLaunchMessage(_ name: String) -> Bool {
    let path = popupTmpDir() + settings.notesSocketName
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    let flags = fcntl(fd, F_GETFL, 0)
    _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)
    var addr = makeUnixSockAddr(path)
    let rc = withUnsafePointer(to: &addr) { ptr -> Int32 in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    if rc != 0 {
        guard errno == EINPROGRESS else { return false }
        var pfd = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
        guard poll(&pfd, 1, Int32(ipcSocketTimeout * 1000)) == 1, pfd.revents & Int16(POLLOUT) != 0 else {
            return false
        }
        var err: Int32 = 0
        var len = socklen_t(MemoryLayout<Int32>.size)
        getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &len)
        guard err == 0 else { return false }
    }
    _ = fcntl(fd, F_SETFL, flags)
    let msg = name + "\n"
    msg.withCString { _ = write(fd, $0, msg.count) }
    return true
}

func writeAll(_ fd: Int32, _ data: Data) {
    data.withUnsafeBytes { raw in
        guard var p = raw.baseAddress else { return }
        var left = raw.count
        while left > 0 {
            let n = Darwin.write(fd, p, left)
            if n <= 0 { if n < 0 && errno == EINTR { continue }; return }
            left -= n
            p += n
        }
    }
}

func sendRequest(_ msg: String, timeout: Double) -> Data? {
    let path = popupTmpDir() + settings.notesSocketName
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return nil }
    defer { close(fd) }
    var addr = makeUnixSockAddr(path)
    let rc = withUnsafePointer(to: &addr) { ptr -> Int32 in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard rc == 0 else { return nil }
    var tv = timeval(tv_sec: Int(timeout), tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
    let line = msg + "\n"
    line.withCString { _ = write(fd, $0, strlen($0)) }
    var out = Data()
    var buf = [UInt8](repeating: 0, count: 65536)
    while true {
        let n = read(fd, &buf, buf.count)
        if n > 0 { out.append(contentsOf: buf[..<n]); continue }
        if n < 0 && errno == EINTR { continue }
        break
    }
    return out
}

private var daemonLockFD: Int32 = -1
func acquireDaemonLock(waitUpTo seconds: Double) -> Bool {
    if daemonLockFD >= 0 { return true }
    let fd = open(popupTmpDir() + settings.notesSocketName + ".lock", O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
    guard fd >= 0 else { return true }
    let deadline = Date().addingTimeInterval(seconds)
    while flock(fd, LOCK_EX | LOCK_NB) != 0 {
        if Date() >= deadline { close(fd); return false }
        usleep(50_000)
    }
    daemonLockFD = fd
    return true
}

func readFocusFile() -> (String?, pid_t?) {
    guard let content = try? String(contentsOfFile: settings.focusFilePath, encoding: .utf8)
    else { return (nil, nil) }
    try? FileManager.default.removeItem(atPath: settings.focusFilePath)
    let parts = content.split(separator: " ")
    guard let wid = parts.first, parts.count > 1,
          let pid = Int32(parts[1]) else {
        return parts.first.map { (String($0), nil) } ?? (nil, nil)
    }
    return (String(wid), pid)
}

func writeUInt32(_ fd: Int32, _ v: UInt32) {
    var v = v.littleEndian
    _ = write(fd, &v, 4)
}

func readN(_ fd: Int32, _ n: Int) -> Data? {
    var data = Data()
    var buf = [UInt8](repeating: 0, count: 4096)
    while data.count < n {
        let got = read(fd, &buf, min(buf.count, n - data.count))
        if got <= 0 { return nil }
        data.append(buf, count: got)
    }
    return data
}

func readUInt32(_ fd: Int32) -> UInt32? {
    guard let d = readN(fd, 4) else { return nil }
    return d.withUnsafeBytes { $0.load(as: UInt32.self) }.littleEndian
}

func liveAerospaceSocket() -> String? {
    let fm = FileManager.default
    if fm.fileExists(atPath: settings.aerospaceSocketPath) { return settings.aerospaceSocketPath }
    let own = "/tmp/bobko.aerospace-\(NSUserName()).sock"
    return fm.fileExists(atPath: own) ? own : nil
}

func aerospaceSocket(_ args: [String], timeout: Double = ipcSocketTimeout) -> String? {
    guard let path = liveAerospaceSocket() else { return nil }
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return nil }
    defer { close(fd) }
    var addr = makeUnixSockAddr(path)
    let ok = withUnsafePointer(to: &addr) { ptr -> Bool in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
        }
    }
    guard ok else { return nil }
    var tv = timeval(tv_sec: Int(timeout), tv_usec: Int32((timeout - Double(Int(timeout))) * 1_000_000))
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
    writeUInt32(fd, 1)
    guard readUInt32(fd) != nil else { return nil }
    let payload: [String: Any] = [
        "args": args, "stdin": "", "windowId": NSNull(), "workspace": NSNull(),
    ]
    guard let data = try? JSONSerialization.data(withJSONObject: payload) else { return nil }
    writeUInt32(fd, UInt32(data.count))
    data.withUnsafeBytes { _ = write(fd, $0.baseAddress, data.count) }
    guard let len = readUInt32(fd), let body = readN(fd, Int(len)) else { return nil }
    guard let obj = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else { return nil }
    return obj["stdout"] as? String ?? ""
}

func aerospaceFallback(_ args: [String]) -> String {
    let candidates = settings.aerospaceCLI
    for c in candidates {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: c)
        p.arguments = args
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = Pipe()
        do { try p.run() } catch { continue }
        let sem = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .userInitiated).async {
            p.waitUntilExit()
            sem.signal()
        }
        if sem.wait(timeout: .now() + ipcFallbackTimeout) == .timedOut {
            p.terminate()
            continue
        }
        if p.terminationStatus == 0 {
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            return String(data: data, encoding: .utf8) ?? ""
        }
    }
    return ""
}

func aerospaceCall(_ args: [String]) -> String {
    let t0 = DispatchTime.now().uptimeNanoseconds
    let r = aerospaceSocket(args) ?? aerospaceFallback(args)
    let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
    if FileManager.default.fileExists(atPath: settings.aeroDebugFlag) {
        appendToFile(settings.aeroLog, String(format: "%.1fms %@ -> [%@]\n", ms, args.joined(separator: " "), r))
    }
    return r
}

struct AppInfo {
    let name: String
    let bundleID: String?
    let windowTitle: String?
}

struct WorkspaceInfo {
    let id: String
    var apps: [AppInfo]
    var focused = false
}

func gatherWorkspaces() -> [WorkspaceInfo] {
    let order = aerospaceCall(["list-workspaces", "--all"])
        .split(separator: "\n").map(String.init)
    let focused = aerospaceCall(["list-workspaces", "--focused"])
        .trimmingCharacters(in: .whitespacesAndNewlines)
    var dict = Dictionary(uniqueKeysWithValues: order.map {
        ($0, WorkspaceInfo(id: $0, apps: [], focused: $0 == focused))
    })
    let wins = aerospaceCall([
        "list-windows", "--all",
        "--format", "%{app-name}|%{app-bundle-id}|%{window-title}|%{workspace}",
    ])
    for line in wins.split(separator: "\n") {
        let parts = line.split(separator: "|").map(String.init)
        guard parts.count == 4, var ws = dict[parts[3]] else { continue }
        ws.apps.append(AppInfo(name: parts[0],
                               bundleID: parts[1].isEmpty ? nil : parts[1],
                               windowTitle: parts[2].isEmpty ? nil : parts[2]))
        dict[parts[3]] = ws
    }
    return order.compactMap { dict[$0] }.sorted { a, b in
        let an = Int(a.id), bn = Int(b.id)
        switch (an, bn) {
        case (nil, nil): return a.id.localizedCaseInsensitiveCompare(b.id) == .orderedAscending
        case (nil, .some): return true
        case (.some, nil): return false
        case (.some, .some): return an! < bn!
        }
    }
}

struct ListColumn {
    var field: String
    var title: String
    var width: CGFloat
    var align: String
    var sortable: Bool
    var filterable: Bool

    static func parse(_ spec: String?) -> [ListColumn] {
        (spec ?? "").split(separator: ",").compactMap { part in
            let seg = part.split(separator: ":", omittingEmptySubsequences: false)
                .map { $0.trimmingCharacters(in: .whitespaces) }
            guard let field = seg.first, !field.isEmpty else { return nil }
            let title = seg.count > 1 && !seg[1].isEmpty ? seg[1] : field
            let width = seg.count > 2 ? CGFloat(Double(seg[2]) ?? 0) : 0
            var align = seg.count > 3 && !seg[3].isEmpty ? seg[3].lowercased() : "left"
            if !["left", "right", "center"].contains(align) { align = "left" }
            let flags = Set(seg.dropFirst(4).flatMap {
                $0.lowercased().split(whereSeparator: { "+/|".contains($0) }).map(String.init)
            })
            return ListColumn(field: field, title: title, width: max(0, width), align: align,
                              sortable: flags.contains("sort"), filterable: flags.contains("filter"))
        }
    }

    static func serialize(_ cols: [ListColumn], titles: Bool = true) -> String {
        cols.map { c in
            let w = c.width == c.width.rounded() ? String(Int(c.width)) : String(format: "%.1f", c.width)
            let flags = [c.filterable ? "filter" : nil, c.sortable ? "sort" : nil]
                .compactMap { $0 }.joined(separator: "+")
            return "\(c.field):\(titles ? c.title : ""):\(w):\(c.align)" + (flags.isEmpty ? "" : ":\(flags)")
        }.joined(separator: ", ")
    }

    var popup: PopupTableColumn {
        PopupTableColumn(field: field, title: title, width: width,
                         align: align == "right" ? .right : align == "center" ? .center : .left,
                         sortable: sortable, filterable: filterable)
    }
}

struct CommandSpec {
    enum Kind { case shell, note, list, output, files }

    var name: String
    var kind: Kind
    var windowName: String
    var chromeTitle: String
    var script: String?
    var paths: [String] = []
    var sources: [String] = []
    var root: String?
    var favorites: [String] = []
    var recent = true
    var recentDays = 7
    var recentLimit = 200
    var recentExclude: [String] = []
    var startRecent = true
    var recentEverywhere = true
    var browserBackground: NSColor?
    var backgroundColor: NSColor?
    var tintAlpha: CGFloat?
    var primary: String?
    var content: String?
    var detail: String?
    var trailing: String?
    var body: String?
    var filter: [String] = []
    var filters: [String] = []
    var width: CGFloat = 0
    var maxRows = 0
    var contentCap = 0
    var bodyLines = 0
    var pageSize = 0
    var copyFields: [String] = []
    var checkbox: Bool?
    var resize = false
    var drag = true
    var sticky = true
    var float: Bool?
    var panel = false
    var label: String?
    var aliases: [String] = []
    var inPalette = true
    var tabsOpaque: Bool?
    var sort: String?
    var sortDescending: Bool?
    var searchLimit: Int?
    var searchExclude: [String]?
    var terminalWords: [String]?
    var searchWidth: CGFloat = 0
    var maxStretch: CGFloat = 0
    var tableRowHeight: CGFloat = 0
    var height: CGFloat = 0
    var maxHeight: CGFloat = 0
    var font: String?
    var fontSize: CGFloat = 0
    var headerColor: NSColor?
    var voice = false
    var voiceLive = true
    var terminal = false
    var terminalHeight: CGFloat = 240
    var inspectorWidth: CGFloat = 340
    var sidebarWidth: CGFloat = 210
    var proseFont: String?
    var proseFontSize: CGFloat = 0
    var proseWidth: CGFloat = 0
    var newNoteName = "Untitled"
    var newDocName = "doc"
    var newDocTemplate = "markdown_doc_catppuccin_latte"
    var notesFolder: String? {
        paths.first.map { p -> String in
            let e = (p as NSString).expandingTildeInPath
            var isDir: ObjCBool = false
            FileManager.default.fileExists(atPath: e, isDirectory: &isDir)
            return isDir.boolValue ? e : (e as NSString).deletingLastPathComponent
        }
    }
    var terminalDir: String?
    var terminalBackground: NSColor?
    var textColor: NSColor?
    var dimColor: NSColor?
    var highlightColor: NSColor?
    var accentColor: NSColor?
    var palette: PopupPalette?
    var terminalForeground: NSColor?
    var vimMode = false
    var vimBin = "nvim"
    var vimInit: String?
    var startDrawer = "none"
    var escClose: Int?
    var imageRows = 10
    var icon: NSImage?
    var saveDir = "/tmp/"
    var table = false
    var columns: [ListColumn] = []
    var tableSort: String?

    init(name: String, kind: Kind = .shell, script: String? = nil) {
        self.name = name
        self.kind = kind
        windowName = name
        chromeTitle = name
        self.script = script
    }
}

struct ShortcutEntry {
    let view: String
    let keys: String
    let what: String
}
var shortcutEntries: [ShortcutEntry] = []

func sharedShortcutGroups() -> [ShortcutRows] {
    func items(_ v: String) -> [(keys: String, what: String)] {
        shortcutEntries.filter { $0.view == v }.map { ($0.keys, $0.what) }
    }
    return [("Sidebars", items("sidebar")), ("Previews & Pages", items("preview")), ("Everywhere", items("all"))]
        .filter { !$0.1.isEmpty }
}

func loadCommands() -> [CommandSpec] {
    applyAppConfigFromDisk()
    guard let content = readConfigText() else {
        FileHandle.standardError.write(Data("ws: \(commandsConfName) missing — no command palette\n".utf8))
        return []
    }
    var cmds: [CommandSpec] = []
    var shortcuts: [ShortcutEntry] = []
    defer { shortcutEntries = shortcuts }
    var section: (name: String, vars: [String: String])?
    func flushSection() {
        guard let s = section else { return }
        switch s.name {
        case "icons":
            iconRules = parseIconRules(s.vars)
        case "shortcuts",
             "app",
             "confluence", "ai", "compare",
             "notifications",
             "pane-shot",
             "notes-find",
             "setup",
             "settings-hub":
            break
        default:
            if s.vars["enabled"] == "true" {
                cmds.append(makeCommand(s.name, s.vars))
            }
        }
        section = nil
    }
    for rec in configDecodedLines(content) {
        let s = rec.trimmed
        if s.isEmpty || s.hasPrefix("#") { continue }
        if let name = rec.header {
            flushSection()
            if !name.isEmpty {
                section = (name, [:])
            }
            continue
        }
        guard let key = rec.key, let val = rec.value else { continue }
        if section?.name == "shortcuts" {
            if let colon = key.firstIndex(of: ":") {
                let view = key[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
                let keys = key[key.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                    .replacingOccurrences(of: "Plus", with: "=").replacingOccurrences(of: "Minus", with: "-")
                if !keys.isEmpty { shortcuts.append(ShortcutEntry(view: view, keys: keys, what: val)) }
            }
            continue
        }
        if section != nil {
            section?.vars[key] = val
        } else if !key.isEmpty && !val.isEmpty {
            cmds.append(CommandSpec(name: key, kind: .shell, script: val))
        }
    }
    flushSection()
    syncJiraLaunchAgent()
    return cmds
}

func jiraEnabledInConfig() -> Bool { jiraConfigFlag("enabled") }

func jiraBackgroundPollInConfig() -> Bool { jiraConfigFlag("poll-when-disabled") }

func jiraPollActiveInConfig() -> Bool { jiraEnabledInConfig() || jiraBackgroundPollInConfig() }

func jiraConfigFlag(_ key: String) -> Bool { tri(jiraConfigValue(key)) == true }

func jiraConfigValue(_ key: String) -> String? { configSectionValue("jira", key) }

func configAliases(_ section: String) -> [String] {
    (configSectionValue(section, "aliases") ?? "").split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
}

func configSectionValue(_ section: String, _ key: String) -> String? {
    guard let content = readConfigText() else { return nil }
    return configSectionEntries(configLines(content), section).first(where: { $0.key == key })?.value
}

func syncJiraLaunchAgent() {
    let enabled = jiraPollActiveInConfig()
    let home = NSHomeDirectory()
    let plist = home + "/Library/LaunchAgents/com.jira.poll.plist"
    let gui = "gui/\(getuid())"
    guard enabled else {
        if FileManager.default.fileExists(atPath: plist) { runLaunchctl(["bootout", gui, plist]) }
        return
    }
    var changed = false
    if let tmpl = try? String(contentsOfFile: assetDir + "/jira/com.jira.poll.plist", encoding: .utf8) {
        let want = tmpl.replacingOccurrences(of: "__WS_CONFIG__",
                                             with: homeDir)
        let have = try? String(contentsOfFile: plist, encoding: .utf8)
        if have != want {
            try? FileManager.default.createDirectory(atPath: home + "/Library/LaunchAgents",
                                                     withIntermediateDirectories: true)
            changed = (try? want.write(toFile: plist, atomically: true, encoding: .utf8)) != nil
        }
    }
    guard FileManager.default.fileExists(atPath: plist) else { return }
    let loaded = runLaunchctl(["print", gui + "/com.jira.poll"]) == 0
    if loaded && !changed { return }
    if loaded { runLaunchctl(["bootout", gui, plist]) }
    runLaunchctl(["bootstrap", gui, plist])
}

@discardableResult
private func runLaunchctl(_ args: [String]) -> Int32 {
    (try? runProcess("/bin/launchctl", args))?.code ?? -1
}

private func makeCommand(_ name: String, _ vars: [String: String]) -> CommandSpec {
    let kinds: [String: CommandSpec.Kind] = ["note": .note, "list": .list, "output": .output, "files": .files]
    var s = CommandSpec(name: name, kind: kinds[vars["type"] ?? ""] ?? .shell, script: vars["script"])
    if let v = vars["name"] { s.windowName = v }
    s.chromeTitle = vars["title"] ?? s.windowName
    if let l = vars["label"]?.trimmingCharacters(in: .whitespaces), !l.isEmpty { s.label = l }
    s.inPalette = tri(vars["in-palette"]) ?? true
    s.aliases = csv(vars["aliases"])
    s.paths = csv(vars["paths"] ?? vars["path"])
    s.root = vars["root"]
    s.favorites = csv(vars["favorites"])
    s.panel = tri(vars["panel"]) ?? false
    s.icon = vars["icon"].flatMap(resolveIconName)
    if let v = vars["save-dir"], !v.isEmpty { s.saveDir = v }
    s.sources = csv(vars["sources"] ?? vars["source"])
    s.primary = vars["primary"]
    s.content = vars["content"]
    s.detail = vars["detail"]
    s.trailing = vars["trailing"]
    s.body = vars["body"]
    s.filter = csv(vars["filter"])
    s.filters = csv(vars["filters"])
    s.maxRows = Int(vars["max-rows"] ?? "") ?? 0
    s.contentCap = Int(vars["content-cap"] ?? "") ?? 0
    s.bodyLines = Int(vars["body-lines"] ?? "") ?? 0
    s.pageSize = Int(vars["page-size"] ?? "") ?? 0
    s.copyFields = csv(vars["copy-fields"])
    s.checkbox = tri(vars["checkbox"])
    s.searchWidth = num(vars["search-width"])
    s.maxStretch = num(vars["max-row-stretch"])
    s.tableRowHeight = num(vars["row-height"])
    s.table = tri(vars["table"]) ?? false
    s.columns = ListColumn.parse(vars["columns"])
    s.tableSort = vars["table-sort"]
    s.width = num(vars["width"])
    s.height = num(vars["height"])
    s.maxHeight = num(vars["max-height"])
    s.resize = tri(vars["resize"]) ?? false
    s.drag = tri(vars["drag"]) ?? true
    s.sticky = tri(vars["sticky"]) ?? true
    s.float = tri(vars["float"])
    s.escClose = Int(vars["esc-close"] ?? vars["vim-esc-close"] ?? "")
    s.font = vars["font"]
    s.fontSize = num(vars["font-size"])
    s.headerColor = hexColor(vars["header-color"])
    s.browserBackground = hexColor(vars["browser-background"])
    s.backgroundColor = hexColor(vars["background-color"])
    if num(vars["tint-alpha"]) > 0 { s.tintAlpha = min(num(vars["tint-alpha"]), 1) }
    s.textColor = hexColor(vars["text-color"])
    s.dimColor = hexColor(vars["dim-color"])
    s.highlightColor = hexColor(vars["highlight-color"])
    s.accentColor = hexColor(vars["accent-color"])
    s.palette = parsePalette(vars["palette"])
    s.tabsOpaque = tri(vars["tabs-opaque"])
    s.voice = tri(vars["voice"]) ?? false
    s.voiceLive = tri(vars["voice-live"]) ?? true
    s.terminal = tri(vars["terminal"]) ?? false
    if num(vars["terminal-height"]) > 0 { s.terminalHeight = num(vars["terminal-height"]) }
    if vars["sidebar-width"] != nil { s.sidebarWidth = num(vars["sidebar-width"]) }
    if vars["inspector-width"] != nil { s.inspectorWidth = num(vars["inspector-width"]) }
    s.proseFont = vars["prose-font"].flatMap { $0.isEmpty ? nil : $0 }
    s.proseFontSize = num(vars["prose-font-size"])
    s.proseWidth = num(vars["prose-width"])
    if let n = vars["new-note-name"]?.trimmingCharacters(in: .whitespaces), !n.isEmpty { s.newNoteName = n }
    if let n = vars["new-doc-name"]?.trimmingCharacters(in: .whitespaces), !n.isEmpty { s.newDocName = n }
    if let n = vars["new-doc-template"]?.trimmingCharacters(in: .whitespaces), !n.isEmpty { s.newDocTemplate = n }
    s.terminalDir = vars["terminal-dir"]
    s.terminalBackground = hexColor(vars["terminal-background"])
    s.terminalForeground = hexColor(vars["terminal-foreground"])
    s.vimMode = tri(vars["vim-mode"]) ?? false
    if let v = vars["vim-bin"] { s.vimBin = v.trimmingCharacters(in: .whitespaces) }
    if let v = vars["vim-init"], !v.isEmpty { s.vimInit = v }
    if let v = vars["start-drawer"], !v.isEmpty { s.startDrawer = v.lowercased() }
    s.imageRows = Int(vars["image-rows"] ?? "") ?? 10
    s.sort = vars["sort"]
    if let o = vars["sort-order"]?.lowercased() { s.sortDescending = o.hasPrefix("desc") }
    s.searchLimit = Int(vars["search-limit"] ?? "")
    if vars["search-exclude"] != nil { s.searchExclude = csv(vars["search-exclude"]) }
    if vars["terminal-words"] != nil { s.terminalWords = csv(vars["terminal-words"]) }
    s.recent = tri(vars["recent"]) ?? true
    s.recentDays = Int(vars["recent-days"] ?? "") ?? 7
    s.recentLimit = Int(vars["recent-limit"] ?? "") ?? 200
    s.recentExclude = csv(vars["recent-exclude"])
    s.startRecent = (vars["start"] ?? "recent").lowercased() != "root"
    s.recentEverywhere = (vars["recent-scope"] ?? "everywhere").lowercased() != "home"
    return s
}

func hexColor(_ s: String?) -> NSColor? {
    guard let s, !s.isEmpty else { return nil }
    var hex = s
    if hex.hasPrefix("0x") { hex = String(hex.dropFirst(2)) }
    if hex.hasPrefix("#") { hex = String(hex.dropFirst(1)) }
    guard hex.count == 6 || hex.count == 8 else { return nil }
    var v: UInt64 = 0
    guard Scanner(string: hex).scanHexInt64(&v) else { return nil }
    let hasAlpha = hex.count == 8
    let a = hasAlpha ? Double((v >> 24) & 0xFF) / 255.0 : 1.0
    return NSColor(srgbRed: Double((v >> 16) & 0xFF) / 255,
                   green: Double((v >> 8) & 0xFF) / 255,
                   blue: Double(v & 0xFF) / 255, alpha: max(a, 0.08))
}

func applyAppConfigFromDisk() {
    guard let content = readConfigText() else { return }
    var vars: [String: String] = [:]
    for e in configSectionEntries(configLines(content), "app") where !e.key.isEmpty {
        vars[e.key] = e.value
    }
    parseAppConfig(vars)
}

private func parseAppConfig(_ vars: [String: String]) {
    let str = { (k: String) -> String? in vars[k]?.trimmingCharacters(in: .whitespaces) }
    let list = { (k: String) -> [String] in
        csv(vars[k]).map { $0.hasPrefix("~") ? ($0 as NSString).expandingTildeInPath : $0 }
    }
    if let v = str("shell"), !v.isEmpty { settings.shell = v }
    if let v = str("terminal-font"), !v.isEmpty { settings.terminalFont = v }
    if let v = str("terminal-font-size"), let n = Double(v), n >= 6 { settings.terminalFontSize = CGFloat(n) }
    let casks = csv(vars["font-install-casks"]).compactMap { entry -> (label: String, cask: String, type: String)? in
        let parts = entry.split(separator: "|").map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count >= 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
        return (parts[0], parts[1], parts.count > 2 ? parts[2].lowercased() : "mono")
    }
    if !casks.isEmpty { settings.fontInstallCasks = casks }
    let sa = (vars["shell-args"] ?? "")
        .split(whereSeparator: { $0 == " " || $0 == "\t" })
        .map(String.init)
    if !sa.isEmpty { settings.shellArgs = sa }
    let cli = list("aerospace-cli")
    if !cli.isEmpty { settings.aerospaceCLI = cli }
    let dirs = list("app-dirs")
    if !dirs.isEmpty { settings.appDirs = dirs }
    if let v = str("jira-icon"), !v.isEmpty { settings.jiraIconName = v }
    if let v = str("confluence-icon"), !v.isEmpty { settings.confluenceIconName = v }
    if let v = str("ai-icon") { settings.aiIconName = v }
    if let v = str("notes-icon"), !v.isEmpty { settings.notesIconName = v }
    if let v = str("app-icon"), !v.isEmpty { settings.appIconName = v }
    if let v = str("files-icon") { settings.filesIconName = v }
    if let v = str("notes-socket"), !v.isEmpty { settings.notesSocketName = v }
    if let v = str("focus-file"), !v.isEmpty { settings.focusFileName = v }
    if let v = str("focus-bridge"), !v.isEmpty { settings.focusBridgeName = v }
    if let v = str("switcher-name"), !v.isEmpty { settings.switcherWindowName = v }
    if let v = str("detail-name"), !v.isEmpty { settings.detailWindowName = v }
    if let v = str("aerospace-socket"), !v.isEmpty {
        settings.aerospaceSocketPath = v.replacingOccurrences(of: "$USER", with: NSUserName())
    }
    if let v = str("crash-log"), !v.isEmpty { settings.crashLogPath = (v as NSString).expandingTildeInPath }
    if let v = str("debug-flag"), !v.isEmpty { settings.aeroDebugFlag = (v as NSString).expandingTildeInPath }
    if let v = str("aero-log"), !v.isEmpty { settings.aeroLog = (v as NSString).expandingTildeInPath }
    if let v = str("voice-locale"), !v.isEmpty { settings.voiceLocale = v }
    if vars["screenshot-apps"] != nil { settings.screenshotApps = csv(vars["screenshot-apps"]) }
    if let v = str("hide-on-focus-loss") { settings.hideOnFocusLoss = tri(v) == true }
    if let v = str("focus-loss-delay"), let n = Double(v), n >= 0 { settings.focusLossDelay = min(n, 5) }
    HeaderStyle.current = str("header-style").flatMap { HeaderStyle(rawValue: $0.lowercased()) } ?? .flat
    if let v = tri(str("shared-window")) { settings.sharedWindow = v }
    if let v = tri(str("preload")) { settings.preload = v }
    if let v = str("shared-width"), let n = Double(v), n >= 400 { settings.sharedWidth = CGFloat(n) }
    if let v = str("shared-height"), let n = Double(v), n >= 300 { settings.sharedHeight = CGFloat(n) }
    if let v = str("margin-top"), let n = Double(v) { settings.marginTop = CGFloat(max(0, n)) }
    if let v = str("switcher-width"), let n = Double(v) { settings.switcherWidth = CGFloat(min(1200, max(240, n))) }
    if let v = str("palette-first") {
        settings.paletteFirst = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty }
    }
    if let v = str("margin-top-builtin"), let n = Double(v) { settings.marginTopBuiltin = CGFloat(max(0, n)) }
    if let v = str("margin-bottom"), let n = Double(v) { settings.marginBottom = CGFloat(max(0, n)) }
    if let v = str("esc-close"), let n = Int(v) { settings.escClose = max(0, n) }
    if vars["copy-toast"] != nil { settings.copyToast = str("copy-toast") ?? "" }
    if let v = str("terminal-app") { settings.terminalApp = v }
    FilePopup.borderColor = hexColor(str("preview-border")) ?? FilePopup.defaultBorder
    FilePopup.borderWidth = str("preview-border-width").flatMap { Double($0) }.map { CGFloat(max(0, min($0, 8))) } ?? 2
    PaneNav.ringColor = hexColor(str("pane-focus-color")) ?? PaneNav.defaultRingColor
    PaneNav.ringWidth = str("pane-focus-width").flatMap { Double($0) }.map { CGFloat(max(0, min($0, 4))) } ?? 1
    VimKeys.enabled = tri(str("vim-keys")) ?? true
    VimKeys.showBadge = tri(str("vim-mode-badge")) ?? true
}

struct ThemeSnapshot {
    let roles: [(PopupWindow.ThemeRole, NSColor)]
    let text, dim, highlight, accent, border: NSColor
    let palette: PopupPalette
    let terminalForeground: NSColor?
    init(_ w: PopupWindow) {
        roles = PopupWindow.ThemeRole.allCases.map { ($0, w.themeColor($0)) }
        text = w.config.colors.text
        dim = w.config.colors.dim
        highlight = w.config.colors.highlight
        accent = w.config.colors.accent
        border = w.config.colors.border
        palette = w.config.colors.palette
        terminalForeground = w.config.terminalForeground
    }
    func restore(_ w: PopupWindow) {
        for (role, c) in roles { w.setThemeColor(c, for: role) }
        w.setTerminalForeground(terminalForeground)
        w.setTextColors(text: text, dim: dim, highlight: highlight, accent: accent,
                        palette: palette, border: border)
    }
}

final class ThemePreviewDelegate: NSObject, NSMenuDelegate {
    private let onHighlight: (Int?) -> Void
    private let onClose: () -> Void
    private var current: Int?
    var committed = false
    init(onHighlight: @escaping (Int?) -> Void, onClose: @escaping () -> Void) {
        self.onHighlight = onHighlight
        self.onClose = onClose
    }
    func menu(_ menu: NSMenu, willHighlight item: NSMenuItem?) {
        let tag = item?.representedObject as? Int
        guard tag != current else { return }
        current = tag
        onHighlight(tag)
    }
    func menuDidClose(_ menu: NSMenu) {
        current = nil
        if !committed { onClose() }
    }
}

final class HeaderStylePreviewDelegate: NSObject, NSMenuDelegate {
    private let original: HeaderStyle
    var committed = false
    init(original: HeaderStyle) { self.original = original }
    func menu(_ menu: NSMenu, willHighlight item: NSMenuItem?) {
        guard !committed else { return }
        let all = HeaderStyle.allCases
        if let t = item?.tag, all.indices.contains(t) { HeaderStyle.current = all[t] }
        else { HeaderStyle.current = original }
    }
    func menuDidClose(_ menu: NSMenu) {
        if !committed { HeaderStyle.current = original }
    }
}

struct ThemePreset {
    let name: String
    let background: NSColor
    let browser: NSColor
    let terminal: NSColor
    let header: NSColor
    let text: NSColor
    let dim: NSColor
    let highlight: NSColor
    let accent: NSColor
    let palette: PopupPalette

    var isLight: Bool { background.relativeLuminance > 0.45 }
    enum Tone: Int { case mid, dark, light }
    var tone: Tone {
        isLight ? .light : background.relativeLuminance >= 0.017 ? .mid : .dark
    }

    static func parse(name: String, _ value: String) -> ThemePreset? {
        let c = value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard !name.isEmpty, [7, 8, 13].contains(c.count) else { return nil }
        let colors = c.compactMap { hexColor($0) }
        guard colors.count == c.count else { return nil }
        return ThemePreset(name: name, background: colors[0], browser: colors[1],
                           terminal: colors[2], header: colors[3], text: colors[4],
                           dim: colors[5], highlight: colors[6],
                           accent: colors.count >= 8 ? colors[7] : colors[4],
                           palette: colors.count == 13 ? PopupPalette(Array(colors[8...]))!
                                                       : ThemePreset.defaultPalette(light: colors[0].relativeLuminance > 0.45))
    }

    static func defaultPalette(light: Bool) -> PopupPalette {
        light ? PopupPalette(colors("1E66F5, 40A02B, DF8E1D, D20F39, 179299"))!
              : PopupPalette()
    }
    private static func colors(_ s: String) -> [NSColor] {
        s.split(separator: ",").compactMap { hexColor($0.trimmingCharacters(in: .whitespaces)) }
    }

    static let builtIn: [ThemePreset] = [
        ("Tokyo Night", "1A1B26, 16161E, 13141C, 111219, C0CAF5, 9AA5CE, 283457, 7AA2F7, BB9AF7, 9ECE6A, E0AF68, F7768E, 7DCFFF"),
        ("Tokyo Night Storm", "24283B, 1F2335, 1B1E2D, 1A1D2B, C0CAF5, 9AA5CE, 2E3C64, 7AA2F7, BB9AF7, 9ECE6A, E0AF68, F7768E, 7DCFFF"),
        ("Ink & Brass", "1A1D24, 171A20, 13161C, 13161C, E7E2D7, 928C80, 2F3440, C9A45C, 7C9CB5, 8DB07A, D8A657, D0705F, 7FA3BF"),
        ("Catppuccin Mocha", "1E1E2E, 181825, 11111B, 11111B, CDD6F4, A6ADC8, 45475A, CBA6F7, 89B4FA, A6E3A1, F9E2AF, F38BA8, 94E2D5"),
        ("Catppuccin Macchiato", "24273A, 1E2030, 181926, 181926, CAD3F5, A5ADCB, 494D64, C6A0F6, 8AADF4, A6DA95, EED49F, ED8796, 8BD5CA"),
        ("Dracula", "282A36, 21222C, 191A21, 191A21, F8F8F2, A4AACC, 44475A, BD93F9, FF79C6, 50FA7B, F1FA8C, FF5555, 8BE9FD"),
        ("Nord", "2E3440, 3B4252, 272C36, 242933, ECEFF4, A3ACBD, 4C566A, 88C0D0, 81A1C1, A3BE8C, EBCB8B, BF616A, 8FBCBB"),
        ("Gruvbox Dark", "282828, 1D2021, 1D2021, 1B1B1B, EBDBB2, A89984, 504945, FABD2F, 83A598, B8BB26, FE8019, FB4934, 8EC07C"),
        ("One Dark", "282C34, 21252B, 1E2127, 1B1E23, ABB2BF, 7F848E, 3E4451, 61AFEF, C678DD, 98C379, E5C07B, E06C75, 56B6C2"),
        ("Rosé Pine", "191724, 1F1D2E, 16141F, 12101A, E0DEF4, 908CAA, 403D52, EBBCBA, C4A7E7, 9CCFD8, F6C177, EB6F92, 31748F"),
        ("Solarized Dark", "002B36, 073642, 00212B, 001E26, 93A1A1, 657B83, 0A4A5A, 268BD2, 6C71C4, 859900, B58900, DC322F, 2AA198"),
        ("Graphite", "1E1E1E, 252525, 181818, 151515, E5E5E5, 9A9A9A, 3A3A3A, 0A84FF, BF5AF2, 30D158, FFD60A, FF453A, 64D2FF"),
        ("Catppuccin Frappé", "303446, 292C3C, 232634, 232634, C6D0F5, A5ADCE, 51576D, CA9EE6, 8CAAEE, A6D189, E5C890, E78284, 81C8BE"),
        ("Tokyo Night Moon", "222436, 1E2030, 191B29, 171927, C8D3F5, 9AA5CE, 2D3F76, 82AAFF, C099FF, C3E88D, FFC777, FF757F, 86E1FC"),
        ("Rosé Pine Moon", "232136, 2A273F, 1D1B2E, 19172A, E0DEF4, 908CAA, 44415A, EA9A97, C4A7E7, 9CCFD8, F6C177, EB6F92, 3E8FB0"),
        ("Everforest Dark", "2D353B, 272E33, 232A2E, 1E2326, D3C6AA, 9DA9A0, 475258, A7C080, D699B6, 83C092, DBBC7F, E67E80, 7FBBB3"),
        ("Palenight", "292D3E, 232635, 1E2130, 1B1E2B, A6ACCD, 8087A2, 444267, C792EA, 82AAFF, C3E88D, FFCB6B, F07178, 89DDFF"),
        ("GitHub Dark Dimmed", "22272E, 1C2128, 1A1E24, 161B22, ADBAC7, 8B98A5, 373E47, 539BF5, DCBDFB, 57AB5A, C69026, E5534B, 96D0FF"),
        ("Ayu Mirage", "1F2430, 1C212B, 171B24, 141820, CCCAC2, 8A9199, 33415E, FFCC66, DFBFFF, D5FF80, FFD173, F28779, 5CCFE6"),
        ("Monokai Pro", "2D2A2E, 221F22, 19181A, 171517, FCFCFA, 939293, 403E41, FFD866, AB9DF2, A9DC76, FC9867, FF6188, 78DCE8"),
        ("Synthwave '84", "262335, 241B2F, 1E1A29, 171520, F0EFF5, 9D98C4, 463465, FF7EDB, 36F9F6, 72F1B8, FEDE5D, FE4450, 03EDF9"),
        ("Kanagawa Wave", "1F1F28, 1A1A22, 16161D, 131318, DCD7BA, C8C093, 2D4F67, 7E9CD8, 957FB8, 98BB6C, E6C384, E46876, 7FB4CA"),
        ("Nightfox", "192330, 131A24, 111720, 0F141C, CDCECF, AEAFB0, 2B3B51, 719CD6, 9D79D6, 81B29A, DBC074, C94F6D, 63CDCF"),
        ("Poimandres", "1B1E28, 171922, 13151D, 111219, E4F0FB, A6ACCD, 303340, 5DE4C7, FCC5E9, 5FB3A1, FFFAC2, D0679D, 89DDFF"),
        ("Night Owl", "011627, 01111D, 010E17, 000C14, D6DEEB, 8BA1B7, 1D3B53, 82AAFF, C792EA, ADDB67, ECC48D, EF5350, 7FDBCA"),
        ("Kanagawa Dragon", "181616, 12120F, 0D0C0C, 0B0A0A, C5C9C5, A6A69C, 2D4F67, 8BA4B0, A292A3, 87A987, C4B28A, C4746E, 8EA4A2"),
        ("Catppuccin Latte", "EFF1F5, E6E9EF, DCE0E8, DCE0E8, 4C4F69, 6C6F85, BCC0CC, 8839EF, 1E66F5, 40A02B, DF8E1D, D20F39, 179299"),
        ("Tokyo Night Day", "E1E2E7, D5D6DB, D0D5E3, C8CCD9, 3760BF, 6172B0, B7C1E3, 2E7DE9, 9854F1, 587539, 8C6C3E, F52A65, 007197"),
        ("Solarized Light", "FDF6E3, EEE8D5, EEE8D5, E4DDC8, 586E75, 839496, DDD6C1, 268BD2, D33682, 859900, B58900, DC322F, 2AA198"),
        ("Paper", "F5F5F5, EDEDED, FFFFFF, E3E3E3, 1D1D1F, 6E6E73, D1D1D6, 007AFF, AF52DE, 248A3D, B25000, D70015, 0071A4"),
    ].compactMap { parse(name: $0.0, $0.1) }

    static func all() -> [ThemePreset] {
        var out = builtIn
        guard let content = readConfigText() else { return out }
        for (_, name, value) in configSectionEntries(configLines(content), "themes") {
            guard let p = parse(name: name, value) else { continue }
            if let i = out.firstIndex(where: { $0.name == name }) { out[i] = p } else { out.append(p) }
        }
        return out
    }

    func swatch() -> NSImage {
        let size = NSSize(width: 58, height: 20)
        return NSImage(size: size, flipped: true) { r in
            let card = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: 4, yRadius: 4)
            self.background.withAlphaComponent(1).setFill()
            card.fill()
            NSGraphicsContext.current?.saveGraphicsState()
            card.addClip()
            self.header.withAlphaComponent(1).setFill()
            NSRect(x: 0, y: 0, width: r.width, height: 7).fill()
            self.accent.setFill()
            NSBezierPath(roundedRect: NSRect(x: 4, y: 1.5, width: 12, height: 4), xRadius: 1.5, yRadius: 1.5).fill()
            self.dim.setFill()
            NSRect(x: 19, y: 3, width: 8, height: 1.2).fill()
            self.terminal.withAlphaComponent(1).setFill()
            NSRect(x: r.width - 14, y: 7, width: 14, height: r.height - 7).fill()
            self.text.setFill()
            NSRect(x: 4, y: 10, width: 18, height: 1.4).fill()
            self.highlight.setFill()
            NSRect(x: 3, y: 13.5, width: r.width - 20, height: 4.5).fill()
            self.accent.setFill()
            NSRect(x: 3, y: 13.5, width: 1.5, height: 4.5).fill()
            for (k, c) in [self.palette.accent2, self.palette.success,
                           self.palette.warning, self.palette.danger].enumerated() {
                c.setFill()
                NSBezierPath(ovalIn: NSRect(x: r.width - 12 + CGFloat(k % 2) * 5,
                                            y: 9 + CGFloat(k / 2) * 5, width: 3.6, height: 3.6)).fill()
            }
            NSGraphicsContext.current?.restoreGraphicsState()
            NSColor.black.withAlphaComponent(0.3).setStroke()
            card.lineWidth = 1
            card.stroke()
            return true
        }
    }
}

extension NSColor {
    var relativeLuminance: CGFloat {
        let c = usingColorSpace(.sRGB) ?? self
        func lin(_ v: CGFloat) -> CGFloat { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(c.redComponent) + 0.7152 * lin(c.greenComponent) + 0.0722 * lin(c.blueComponent)
    }
}

struct ConfigIssue {
    let line: Int
    let message: String
    let fatal: Bool
}

var configBackupPath: String { settings.commandsConfPath + ".bak" }
private(set) var configIssues: [ConfigIssue] = []
private(set) var configUsingBackup = false
private var configReadCache: (stamp: String, text: String?)?

func wsLog(_ s: String) {
    let line = "ws: \(s)\n"
    FileHandle.standardError.write(Data(line.utf8))
    appendToFile(debugLogPath, line)
}

private let configBoolKeys: Set<String> = [
    "enabled", "resize", "drag", "sticky", "voice", "voice-live", "terminal", "vim-mode", "recent",
    "checkbox", "hide-on-focus-loss", "float", "table", "shared-window", "preload", "in-palette",
    "panel", "vim-keys", "vim-mode-badge",
    "show-help", "show-side-panel-button", "show-size-badge", "magnifier", "square-magnifier",
    "copy-on-double-click", "save-path-fixed", "save-after-copy", "copy-path-after-save",
    "save-last-region", "reverse-arrow", "counter-outline", "insecure-pixelate",
]
private let configNumberKeys: [String: ClosedRange<Double>] = [
    "limit": 1...25,
    "width": 100...8000, "height": 60...8000, "max-height": 60...8000,
    "shared-width": 400...8000, "shared-height": 300...8000, "margin-top": 0...400, "margin-top-builtin": 0...400, "margin-bottom": 0...400, "switcher-width": 240...1200, "preview-border-width": 0...8, "pane-focus-width": 0...4,
    "terminal-height": 40...4000, "sidebar-width": 0...600, "inspector-width": 0...800, "prose-font-size": 8...48, "prose-width": 300...2000, "font-size": 6...96, "terminal-font-size": 6...96,
    "max-rows": 0...10_000, "page-size": 0...100_000, "content-cap": 0...100_000,
    "body-lines": 0...100, "search-width": 0...1, "recent-days": 1...365, "recent-limit": 20...5000,
    "tint-alpha": 0...1, "max-row-stretch": 0...1000, "row-height": 18...80, "image-rows": 1...200,
    "vim-esc-close": 0...20, "esc-close": 0...20, "search-limit": 1...1_000_000,
    "dashboard-width": 600...8000, "dashboard-height": 400...8000, "dashboard-refresh": 2...3600,
    "split": 0.2...0.8, "context-tokens": 512...1_000_000, "focus-loss-delay": 0...5,
    "contrast-opacity": 0...255, "jpeg-quality": 1...100, "undo-limit": 1...1000,
    "button-size": 0...80, "arrow-style": 0...1, "delay": 0...60_000,
    "context-lines": 0...1000, "tab-width": 1...16, "time-tolerance": 0...86_400, "max-lines": 1000...5_000_000,
    "max-bytes": 1_048_576...2_147_483_648,
]
private let configColorKeys: Set<String> = [
    "header-color", "background-color", "browser-background", "terminal-background",
    "text-color", "dim-color", "highlight-color", "accent-color", "terminal-foreground",
]
private let configEnumKeys: [String: Set<String>] = [
    "type": ["shell", "note", "list", "output", "files"],
    "start-drawer": ["browser", "terminal", "none"],
    "sort": ["name", "modified", "created", "size", "kind"],
    "sort-order": ["asc", "desc", "ascending", "descending"],
    "header-style": Set(HeaderStyle.allCases.map(\.rawValue)),
]

private func configValueProblem(section: String, key: String, value: String) -> String? {
    guard !value.isEmpty else { return nil }
    if section == "theme" || (section == "app" && (key == "pane-focus-color" || key == "preview-border")) {
        return hexColor(value) == nil ? "'\(value)' is not a hex color (RRGGBB / AARRGGBB)" : nil
    }
    if section == "themes" {
        return ThemePreset.parse(name: key, value) == nil
            ? "expected 7, 8 or 13 hex colors: background, browser, terminal, header, text, dim, highlight[, accent[, accent2, success, warning, danger, info]]"
            : nil
    }
    if key == "favorites" {
        for e in value.split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) where !e.isEmpty {
            var d: ObjCBool = false
            if !(FileManager.default.fileExists(atPath: (e as NSString).expandingTildeInPath, isDirectory: &d) && d.boolValue) {
                return "'\(e)' is not a folder (separate favorites with commas)"
            }
        }
        return nil
    }
    if section == "pane-shot" {
        switch key {
        case "background":
            return AnsiRGB(hex: value) == nil ? "'\(value)' is not a hex color (#RRGGBB)" : nil
        case "lines":
            guard let n = Int(value), (0...Herdr.maxLines).contains(n) else { return "\(value): 0…\(Herdr.maxLines) (herdr's cap)" }
            return nil
        default: break
        }
    }
    if section == "compare" {
        switch key {
        case "content":
            return ["auto", "always", "never"].contains(value.lowercased()) ? nil : "'\(value)' is not one of auto | always | never"
        case "gutter-arrows":
            return ["hover", "always", "off"].contains(value.lowercased()) ? nil : "'\(value)' is not one of hover | always | off"
        case "recent":
            guard let n = Int(value), (0...500).contains(n) else { return "\(value): 0…500 (recent pairs kept)" }
            return nil
        case "ignore-leading-ws", "ignore-trailing-ws", "ignore-embedded-ws", "ignore-case",
             "ignore-line-endings", "ignore-blank-lines", "use-gitignore":
            return tri(value) == nil ? "'\(value)' is not true/false" : nil
        default: break
        }
    }
    if section == "screenshot" {
        switch key {
        case "return":
            return ["copy", "save", "pin"].contains(value.lowercased()) ? nil : "'\(value)' is not one of copy | pin | save"
        case "start-mode":
            return ["screenshot", "text"].contains(value.lowercased()) ? nil : "'\(value)' is not one of screenshot | text"
        case "save-format":
            return ["png", "jpg", "jpeg"].contains(value.lowercased()) ? nil : "'\(value)' is not one of jpg | png"
        case "button-size":
            if let n = Double(value), n > 0 && n < 20 { return "\(value): 0 (automatic) or 20…80" }
        case "ui-color", "contrast-color", "draw-color":
            if value.lowercased() == "theme" && key == "ui-color" { return nil }
            return ShotColor(hex: value) == nil ? "'\(value)' is not a hex color (#RRGGBB)" : nil
        case "user-colors":
            let bad = value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { $0.lowercased() != "picker" && ShotColor(hex: $0) == nil }
            return bad.isEmpty ? nil : "not hex colors: \(bad.joined(separator: ", "))"
        case "buttons":
            let bad = value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces).lowercased() }
                .filter { ShotTool(rawValue: $0) == nil }
            return bad.isEmpty ? nil : "unknown buttons: \(bad.joined(separator: ", "))"
        default: break
        }
    }
    if configBoolKeys.contains(key), tri(value) == nil {
        return "'\(value)' is not true/false"
    }
    if let range = configNumberKeys[key] {
        guard let n = Double(value) else { return "'\(value)' is not a number" }
        if !range.contains(n) {
            return "\(value) is outside \(range.lowerBound.clean)…\(range.upperBound.clean)"
        }
    }
    if key == "palette", parsePalette(value) == nil {
        return "expected 5 hex colors: accent2, success, warning, danger, info"
    }
    if configColorKeys.contains(key), hexColor(value) == nil {
        return "'\(value)' is not a hex color (RRGGBB / AARRGGBB)"
    }
    if let allowed = configEnumKeys[key], !allowed.contains(value.lowercased()) {
        return "'\(value)' is not one of \(allowed.sorted().joined(separator: " | "))"
    }
    if key == "columns" {
        for part in value.split(separator: ",") {
            let seg = part.split(separator: ":", omittingEmptySubsequences: false)
                .map { $0.trimmingCharacters(in: .whitespaces) }
            if seg.first?.isEmpty ?? true { return "an entry has no field name" }
            if seg.count > 2, !seg[2].isEmpty, Double(seg[2]) == nil {
                return "'\(seg[0])' width '\(seg[2])' is not a number (percent)"
            }
            if seg.count > 3, !seg[3].isEmpty, !["left", "right", "center"].contains(seg[3].lowercased()) {
                return "'\(seg[0])' align '\(seg[3])' is not left | right | center"
            }
            for f in seg.dropFirst(4).flatMap({ $0.lowercased().split(whereSeparator: { "+/|".contains($0) }) })
            where f != "filter" && f != "sort" {
                return "'\(seg[0])' flag '\(f)' is not filter | sort"
            }
        }
        let total = ListColumn.parse(value).reduce(CGFloat(0)) { $0 + $1.width }
        if total > 100.5 {
            return "column widths add up to \(Int(total))% (> 100 — they are scaled to fit)"
        }
    }
    return nil
}

private extension Double {
    var clean: String { self == rounded() ? String(Int(self)) : String(self) }
}

func validateConfig(_ text: String) -> [ConfigIssue] {
    var issues: [ConfigIssue] = []
    func warn(_ n: Int, _ m: String) { issues.append(ConfigIssue(line: n, message: m, fatal: false)) }
    func fail(_ n: Int, _ m: String) { issues.append(ConfigIssue(line: n, message: m, fatal: true)) }
    if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        fail(0, "the file is empty")
        return issues
    }
    if text.contains("\u{0}") {
        fail(0, "the file contains binary (NUL) data")
        return issues
    }
    var section: String?
    var seenSections: Set<String> = []
    var seenKeys: Set<String> = []
    var entries = 0
    var garbage = 0
    for rec in configDecodedLines(text) {
        let n = rec.index + 1
        let s = rec.trimmed
        if s.isEmpty || s.hasPrefix("#") { continue }
        if s.hasPrefix("[") {
            let name = rec.header ?? ""
            guard !name.isEmpty, !name.contains("["), !name.contains("]") else {
                fail(n, "malformed section header '\(s.prefix(40))' (expected [name])")
                continue
            }
            if seenSections.contains(name) {
                warn(n, "duplicate section [\(name)]")
            }
            seenSections.insert(name)
            section = name
            seenKeys = []
            continue
        }
        guard let key = rec.key, let val = rec.value else {
            garbage += 1
            warn(n, "ignored line (no '='): \(s.prefix(40))")
            continue
        }
        guard !key.isEmpty else {
            garbage += 1
            warn(n, "missing key before '='")
            continue
        }
        entries += 1
        guard let sec = section else { continue }
        if seenKeys.contains(key) {
            warn(n, "[\(sec)] duplicate key '\(key)' (the last one wins)")
        }
        seenKeys.insert(key)
        if let problem = configValueProblem(section: sec, key: key, value: val) {
            warn(n, "[\(sec)] \(key): \(problem)")
        }
    }
    if entries == 0 {
        fail(0, "no `key = value` entries")
    } else if garbage > max(5, entries) {
        fail(0, "\(garbage) unparseable lines — the file looks corrupted")
    }
    return issues
}

func configSchemaJSON() -> String {
    let obj: [String: Any] = [
        "version": 1,
        "boolKeys": configBoolKeys.sorted(),
        "numberRanges": configNumberKeys.mapValues { [$0.lowerBound, $0.upperBound] },
        "colorKeys": configColorKeys.sorted(),
        "enumKeys": configEnumKeys.mapValues { $0.sorted() },
    ]
    let data = (try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])) ?? Data("{}".utf8)
    return String(decoding: data, as: UTF8.self)
}

func configCheckCLI(_ args: [String]) -> Int32 {
    func emit(_ obj: [String: Any]) {
        let data = (try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])) ?? Data("{}".utf8)
        print(String(decoding: data, as: UTF8.self))
    }
    if args.count == 2, args[0] == "--file" {
        guard let text = try? String(contentsOfFile: (args[1] as NSString).expandingTildeInPath, encoding: .utf8) else {
            emit(["ok": false, "issues": [["line": 0, "message": "can't read \(args[1])", "fatal": true]]])
            return 1
        }
        let issues = validateConfig(text)
        emit(["ok": !issues.contains(where: \.fatal),
              "issues": issues.map { ["line": $0.line, "message": $0.message, "fatal": $0.fatal] }])
        return 0
    }
    guard args.count == 3 else {
        FileHandle.standardError.write(Data("usage: kitchen-sink config-check SECTION KEY VALUE | --file PATH\n".utf8))
        return 2
    }
    if let problem = configValueProblem(section: args[0], key: args[1], value: args[2]) {
        emit(["ok": false, "problem": problem])
    } else {
        emit(["ok": true])
    }
    return 0
}

func readConfigText() -> String? {
    let path = settings.commandsConfPath
    let attrs = try? FileManager.default.attributesOfItem(atPath: path)
    let mtime = (attrs?[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
    let stamp = "\(mtime)-\((attrs?[.size] as? Int) ?? -1)"
    if let c = configReadCache, c.stamp == stamp { return c.text }

    let text = try? String(contentsOfFile: path, encoding: .utf8)
    let backup = try? String(contentsOfFile: configBackupPath, encoding: .utf8)
    var issues = text.map(validateConfig) ?? [ConfigIssue(
        line: 0,
        message: attrs == nil ? "\(commandsConfName) is missing" : "\(commandsConfName) is not readable UTF-8 text",
        fatal: true)]
    var result = text
    var usingBackup = false
    if issues.contains(where: \.fatal) {
        if let backup, !validateConfig(backup).contains(where: \.fatal) {
            result = backup
            usingBackup = true
            if attrs == nil {
                try? backup.write(toFile: path, atomically: true, encoding: .utf8)
                issues = [ConfigIssue(line: 0, message: "\(commandsConfName) was missing — restored from backup", fatal: false)]
                usingBackup = false
            }
            let why = issues.filter(\.fatal)
                .map { ($0.line > 0 ? "line \($0.line): " : "") + $0.message }
                .joined(separator: "; ")
            wsLog("commands.toml invalid (\(why)) — using \(configBackupPath)")
        }
        issues = issues.filter(\.fatal) + issues.filter { !$0.fatal && attrs == nil }
    } else if let text, text != backup {
        try? text.write(toFile: configBackupPath, atomically: true, encoding: .utf8)
    }
    for i in issues where !i.fatal { wsLog("commands.toml:\(i.line): \(i.message)") }
    configIssues = issues
    configUsingBackup = usingBackup
    let a2 = try? FileManager.default.attributesOfItem(atPath: path)
    let m2 = (a2?[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
    configReadCache = ("\(m2)-\((a2?[.size] as? Int) ?? -1)", result)
    return result
}

@discardableResult
func writeConfigText(_ text: String) -> Bool {
    let fatal = validateConfig(text).filter(\.fatal)
    guard fatal.isEmpty else {
        wsLog("commands.toml: write refused — \(fatal.map(\.message).joined(separator: "; "))")
        return false
    }
    let path = settings.commandsConfPath
    if let cur = try? String(contentsOfFile: path, encoding: .utf8), cur != text {
        if validateConfig(cur).contains(where: \.fatal) {
            let parked = path + ".broken-\(Int(Date().timeIntervalSince1970))"
            try? cur.write(toFile: parked, atomically: true, encoding: .utf8)
            wsLog("commands.toml: invalid file parked at \(parked)")
        } else {
            try? cur.write(toFile: configBackupPath, atomically: true, encoding: .utf8)
        }
    }
    do {
        try text.write(toFile: path, atomically: true, encoding: .utf8)
        return true
    } catch {
        wsLog("commands.toml: write failed: \(error)")
        return false
    }
}

func restoreConfigFromBackup() -> Bool {
    guard let bak = try? String(contentsOfFile: configBackupPath, encoding: .utf8) else { return false }
    return writeConfigText(bak)
}

func saveConfigValue(section: String, key: String, value: String) {
    saveConfigValues(section: section, [(key, value)])
}

func saveConfigValues(section: String, _ kv: [(String, String?)]) {
    guard let content = readConfigText() else { return }
    guard let text = configSettingText(content, section: section, kv) else { return }
    writeConfigText(text)
}

func removeConfigValue(section: String, key: String) {
    saveConfigValues(section: section, [(key, nil)])
}

private func parseIconRules(_ vars: [String: String]) -> [IconRule] {
    var rules: [IconRule] = []
    for (app, spec) in vars {
        var dflt: NSImage?
        var matches: [(match: String, icon: NSImage)] = []
        for token in spec.split(separator: ",") {
            let parts = token.trimmingCharacters(in: .whitespaces)
                .split(separator: ":", maxSplits: 1).map(String.init)
            guard parts.count == 2, let img = resolveIconName(
                parts[1].trimmingCharacters(in: .whitespaces)) else { continue }
            let match = parts[0].trimmingCharacters(in: .whitespaces).lowercased()
            if match == "*" || match.isEmpty {
                dflt = img
            } else {
                matches.append((match, img))
            }
        }
        rules.append(IconRule(app: app, defaultIcon: dflt, titleMatches: matches))
    }
    return rules
}

private func resolveIconName(_ name: String) -> NSImage? {
    switch name.lowercased() {
    case "jira": return jiraAppIcon
    case "notes", "note": return notesAppIcon
    case "app": return appIcon
    case "heart": return heartIcon
    case "mic", "voice": return micIcon
    case "folder", "files", "paths": return filesNavIcon
    default:
        let p = name.hasPrefix("/") ? name : assetDir + "/" + name
        return fileIconTile(p, size: appIconSize)
    }
}

private func num(_ s: String?) -> CGFloat {
    CGFloat(Double(s ?? "") ?? 0)
}

private func csv(_ s: String?) -> [String] {
    (s ?? "").split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .filter { !$0.isEmpty }
}

private func mtime(of path: String) -> Date? {
    guard let attrs = try? FileManager.default.attributesOfItem(atPath: path) else { return nil }
    return attrs[.modificationDate] as? Date
}

private func noteIsPreview(_ path: String) -> Bool {
    let ext = (path as NSString).pathExtension.lowercased()
    if ext == "pdf" || ext == "rtf" { return true }
    return ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff"].contains(ext)
}

final class TextFieldSheet: NSObject {
    static var live: [TextFieldSheet] = []
    private let field: NSTextField
    private let panel: NSWindow
    private let sheet: NSWindow
    private let onResult: (String?) -> Void
    init(panel: NSWindow, field: NSTextField, sheet: NSWindow,
         onResult: @escaping (String?) -> Void) {
        self.field = field
        self.panel = panel
        self.sheet = sheet
        self.onResult = onResult
    }
    @objc func ok(_ sender: Any?) { dismiss(field.stringValue) }
    @objc func cancel(_ sender: Any?) { dismiss(nil) }
    private func dismiss(_ value: String?) {
        TextFieldSheet.live.removeAll { $0 === self }
        panel.endSheet(sheet)
        onResult(value)
    }
}

func presentPathSheet(on panel: NSWindow,
                      title: String,
                      message: String,
                      okTitle: String,
                      onResult: @escaping (String?) -> Void) {
    let field = NSTextField(frame: NSRect(x: 16, y: 60, width: 428, height: 24))
    field.usesSingleLineMode = true
    field.cell?.wraps = false
    field.cell?.isScrollable = true
    field.font = NSFont.systemFont(ofSize: 13)

    let ok = NSButton(title: okTitle, target: nil, action: nil)
    ok.bezelStyle = .rounded
    ok.keyEquivalent = "\r"
    let cancel = NSButton(title: "Cancel", target: nil, action: nil)
    cancel.bezelStyle = .rounded
    cancel.keyEquivalent = "\u{1b}"

    let label = NSTextField(labelWithString: message)
    label.font = NSFont.systemFont(ofSize: 12)
    label.textColor = .secondaryLabelColor

    let sheet = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 460, height: 128),
                         styleMask: [.titled], backing: .buffered, defer: false)
    sheet.title = title
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 460, height: 128))
    label.frame = NSRect(x: 16, y: 98, width: 428, height: 18)
    field.frame = NSRect(x: 16, y: 62, width: 428, height: 24)
    ok.frame = NSRect(x: 460 - 16 - 88, y: 18, width: 88, height: 30)
    cancel.frame = NSRect(x: ok.frame.minX - 88 - 8, y: 18, width: 88, height: 30)
    content.addSubview(label)
    content.addSubview(field)
    content.addSubview(ok)
    content.addSubview(cancel)
    sheet.contentView = content
    sheet.initialFirstResponder = field

    let target = TextFieldSheet(panel: panel, field: field, sheet: sheet, onResult: onResult)
    TextFieldSheet.live.append(target)
    ok.target = target
    ok.action = #selector(TextFieldSheet.ok(_:))
    cancel.target = target
    cancel.action = #selector(TextFieldSheet.cancel(_:))

    panel.makeKeyAndOrderFront(nil)
    DispatchQueue.main.async {
        panel.beginSheet(sheet)
        sheet.makeFirstResponder(field)
    }
}

enum DismissedNotes {
    private static let store = NSHomeDirectory() + "/.cache/kitchen-sink/dismissed-notes.json"
    private static var map: Set<String> = {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: store)),
              let arr = try? JSONSerialization.jsonObject(with: data) as? [String] else {
            return []
        }
        return Set(arr)
    }()
    private static func norm(_ path: String) -> String {
        (path as NSString).standardizingPath
    }
    static func contains(_ path: String) -> Bool { map.contains(norm(path)) }
    static func add(_ path: String) {
        map.insert(norm(path))
        persist()
    }
    static func remove(_ path: String) {
        guard map.remove(norm(path)) != nil else { return }
        persist()
    }
    private static func persist() {
        try? FileManager.default.createDirectory(
            atPath: (store as NSString).deletingLastPathComponent,
            withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: Array(map)) {
            try? data.write(to: URL(fileURLWithPath: store))
        }
    }
}

private func expandPaths(_ entries: [String], extensions: [String]? = nil) -> [String] {
    var out: [String] = []
    for raw in entries {
        let p = (raw as NSString).expandingTildeInPath
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: p, isDirectory: &isDir) else {
            out.append(p)
            continue
        }
        if isDir.boolValue {
            let items = (try? FileManager.default.contentsOfDirectory(atPath: p)) ?? []
            let files = items
                .filter { !$0.hasPrefix(".") }
                .filter { f in
                    guard let ext = extensions else { return true }
                    return ext.contains((f as NSString).pathExtension.lowercased())
                }
                .sorted()
                .map { (p as NSString).appendingPathComponent($0) }
            out.append(contentsOf: files)
        } else {
            out.append(p)
        }
    }
    var seen = Set<String>()
    return out.filter { seen.insert($0).inserted }
}

final class CommandRunner {
    private let proc: Process
    private let wFd: Int32
    private var output = ""
    private var pending: [String: (String) -> Void] = [:]
    private let q = DispatchQueue(label: "ws.command-runner")

    init?() {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: settings.shell)
        let inp = Pipe()
        let outp = Pipe()
        p.standardInput = inp
        p.standardOutput = outp
        p.standardError = outp
        do { try p.run() } catch { return nil }
        proc = p
        wFd = inp.fileHandleForWriting.fileDescriptor
        let rFd = outp.fileHandleForReading.fileDescriptor
        DispatchQueue.global(qos: .utility).async { [weak self] in
            var buf = [UInt8](repeating: 0, count: 4096)
            while true {
                let n = read(rFd, &buf, buf.count)
                if n <= 0 { return }
                self?.append(String(decoding: buf[..<n], as: UTF8.self))
            }
        }
    }

    func run(_ script: String, completion: @escaping (String) -> Void) {
        q.async { [weak self] in
            guard let self else { return }
            let token = "__WS_DONE_\(UUID().uuidString)__"
            self.pending[token] = completion
            let line = "( \(script) ; echo \"\(token)\" ) 2>&1\n"
            let data = Data(line.utf8)
            data.withUnsafeBytes { _ = write(self.wFd, $0.baseAddress, data.count) }
        }
    }

    private func append(_ s: String) {
        q.sync {
            output += s
            for (token, completion) in pending {
                if let range = output.range(of: token) {
                    let result = String(output[..<range.lowerBound])
                    output.removeSubrange(output.startIndex..<range.upperBound)
                    pending.removeValue(forKey: token)
                    DispatchQueue.main.async { completion(result) }
                }
            }
        }
    }
}

struct IconRule {
    let app: String
    let defaultIcon: NSImage?
    let titleMatches: [(match: String, icon: NSImage)]
}
var iconRules: [IconRule] = []

let appIconSize: CGFloat = 22

let rowPillH: CGFloat = 24
let rowPillBorder: CGFloat = 2
let rowIconSize: CGFloat = 22
let rowIconStride: CGFloat = 26
let rowMaxIcons = 3

let missingIcon: NSImage = {
    let img = NSImage(size: NSSize(width: appIconSize, height: appIconSize))
    img.lockFocus()
    let rect = NSRect(x: 0, y: 0, width: appIconSize, height: appIconSize)
    let path = NSBezierPath(roundedRect: rect, xRadius: 5, yRadius: 5)
    GROUP_BG.setFill()
    path.fill()
    BORDER.setStroke()
    path.lineWidth = 1
    path.stroke()
    let attrs: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 14), .foregroundColor: TEXT,
    ]
    let s = "?" as NSString
    let sz = s.size(withAttributes: attrs)
    s.draw(at: NSPoint(x: (appIconSize - sz.width) / 2, y: (appIconSize - sz.height) / 2),
           withAttributes: attrs)
    img.unlockFocus()
    return img
}()

let jiraAccent = NSColor(red: 0.36, green: 0.62, blue: 0.95, alpha: 1)

func glyphIcon(_ symbol: String, fallback: String, tint: NSColor,
               size: CGFloat = appIconSize, template: Bool = false,
               tile: Bool = true) -> NSImage {
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()
    let rect = NSRect(x: 0, y: 0, width: size, height: size)
    let ink = template ? NSColor.black : tint
    if tile && !template {
        let path = NSBezierPath(roundedRect: rect, xRadius: 5, yRadius: 5)
        GROUP_BG.setFill()
        path.fill()
        BORDER.setStroke()
        path.lineWidth = 1
        path.stroke()
    }
    if let sym = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
        .withSymbolConfiguration(
            NSImage.SymbolConfiguration(pointSize: size * 0.62, weight: .medium)
                .applying(NSImage.SymbolConfiguration(paletteColors: [ink]))) {
        sym.draw(in: rect.insetBy(dx: 2, dy: 2), from: .zero,
                 operation: .sourceOver, fraction: 1)
    } else {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: size * 0.6, weight: .semibold),
            .foregroundColor: ink,
        ]
        let s = fallback as NSString
        let sz = s.size(withAttributes: attrs)
        s.draw(at: NSPoint(x: (size - sz.width) / 2, y: (size - sz.height) / 2),
               withAttributes: attrs)
    }
    img.unlockFocus()
    img.isTemplate = template
    return img
}

func notepadIcon(size: CGFloat) -> NSImage {
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()
    let r = NSRect(x: 0, y: 0, width: size, height: size)
    let paper = NSBezierPath(roundedRect: r.insetBy(dx: size * 0.08, dy: size * 0.06),
                             xRadius: size * 0.12, yRadius: size * 0.12)
    NSColor(red: 0.96, green: 0.94, blue: 0.88, alpha: 1).setFill()
    paper.fill()
    let bh = size * 0.20
    let strip = NSRect(x: r.minX + size * 0.08, y: r.maxY - size * 0.06 - bh,
                       width: size * 0.84, height: bh)
    NSColor(red: 0.17, green: 0.70, blue: 0.64, alpha: 1).setFill()
    NSBezierPath(roundedRect: strip, xRadius: size * 0.05, yRadius: size * 0.05).fill()
    NSColor(red: 0.96, green: 0.94, blue: 0.88, alpha: 1).setFill()
    for i in 0..<4 {
        let cx = strip.minX + strip.width * (CGFloat(i) + 0.5) / 4
        let d = size * 0.07
        NSBezierPath(ovalIn: NSRect(x: cx - d / 2, y: strip.midY - d / 2,
                                    width: d, height: d)).fill()
    }
    NSColor(red: 0.35, green: 0.45, blue: 0.60, alpha: 1).setFill()
    for i in 0..<3 {
        let y = strip.minY - size * 0.12 - CGFloat(i) * size * 0.14
        let w = size * (i == 2 ? 0.42 : 0.58)
        NSBezierPath(roundedRect: NSRect(x: r.minX + size * 0.18, y: y,
                                         width: w, height: size * 0.06),
                     xRadius: size * 0.03, yRadius: size * 0.03).fill()
    }
    let fold = NSBezierPath()
    fold.move(to: NSPoint(x: r.maxX - size * 0.08, y: r.minY + size * 0.06))
    fold.line(to: NSPoint(x: r.maxX - size * 0.30, y: r.minY + size * 0.06))
    fold.line(to: NSPoint(x: r.maxX - size * 0.08, y: r.minY + size * 0.28))
    fold.close()
    NSColor(red: 0.95, green: 0.65, blue: 0.25, alpha: 1).setFill()
    fold.fill()
    img.unlockFocus()
    return img
}

let notesAppIcon = fileIconTile(settings.notesIconPath, size: appIconSize)
    ?? notepadIcon(size: appIconSize)
let jiraAppIcon = fileIconTile(settings.jiraIconPath, size: appIconSize)
    ?? glyphIcon("ticket", fallback: "J", tint: jiraAccent)
let appIcon = fileIconTile(settings.appIconPath, size: appIconSize)
    ?? notepadIcon(size: appIconSize)
let notesNavIcon: NSImage = NSImage(contentsOfFile: settings.notesIconPath) ?? notepadIcon(size: 32)
let jiraNavIcon: NSImage = NSImage(contentsOfFile: settings.jiraIconPath) ?? jiraAppIcon
let confluenceAppIcon = fileIconTile(settings.confluenceIconPath, size: appIconSize)
    ?? glyphIcon("book.pages", fallback: "C", tint: jiraAccent)
let confluenceNavIcon: NSImage = NSImage(contentsOfFile: settings.confluenceIconPath) ?? confluenceAppIcon
let aiTint = NSColor(srgbRed: 0.72, green: 0.56, blue: 0.98, alpha: 1)
let aiAppIcon = (settings.aiIconPath.isEmpty ? nil : fileIconTile(settings.aiIconPath, size: appIconSize))
    ?? glyphIcon("sparkles", fallback: "✦", tint: aiTint)
let aiNavIcon: NSImage = (settings.aiIconPath.isEmpty ? nil : NSImage(contentsOfFile: settings.aiIconPath))
    ?? glyphIcon("sparkles", fallback: "✦", tint: aiTint, size: 32, tile: false)
let filesNavIcon: NSImage = (settings.filesIconPath.isEmpty ? nil : NSImage(contentsOfFile: settings.filesIconPath))
    ?? NSImage(named: NSImage.folderName) ?? notepadIcon(size: 32)
let heartIcon = glyphIcon("heart.fill", fallback: "♥",
                          tint: NSColor.systemRed.withAlphaComponent(0.9),
                          tile: false)
let micIcon = glyphIcon("mic.fill", fallback: "🎙",
                        tint: NSColor.systemRed.withAlphaComponent(0.9),
                        tile: false)
let utilityMenuGlyph: NSImage = {
    let sym = NSImage(systemSymbolName: "wrench.and.screwdriver",
                      accessibilityDescription: nil)?
        .withSymbolConfiguration(
            NSImage.SymbolConfiguration(pointSize: 15, weight: .medium))
    let img = sym ?? glyphIcon("gearshape", fallback: "⚙", tint: jiraAccent,
                               size: 18, template: true)
    img.isTemplate = sym != nil
    return img
}()

func fileIconTile(_ path: String, size: CGFloat) -> NSImage? {
    guard let src = NSImage(contentsOfFile: path) else { return nil }
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()
    src.draw(in: NSRect(x: 0, y: 0, width: size, height: size),
             from: .zero, operation: .sourceOver, fraction: 1)
    img.unlockFocus()
    return img
}

var jiraSite: String {
    if let data = try? Data(contentsOf: URL(fileURLWithPath: JiraPoll.configPath)),
       let d = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
       let site = d["site"] as? String, !site.isEmpty {
        return site.hasSuffix("/") ? String(site.dropLast()) : site
    }
    guard let s = try? String(contentsOfFile: JiraPoll.paths.legacyConfig, encoding: .utf8) else { return "" }
    for line in s.split(separator: "\n") where line.hasPrefix("JIRA_SITE=") {
        return String(line.dropFirst("JIRA_SITE=".count))
            .trimmingCharacters(in: CharacterSet(charactersIn: "'\""))
    }
    return ""
}

let jiraReleasesWindow = "jira-releases"

func jiraIsReleaseRow(_ r: FieldRow) -> Bool {
    let f = r.fields
    if f["versionId"] != nil { return true }
    let rel = f["release"] ?? "", proj = f["project"] ?? ""
    return !rel.isEmpty && !proj.isEmpty && f["key"] == "\(proj)-\(rel)" && r.title == rel
}

func jiraBrowseURL(_ r: FieldRow, site: String = jiraSite) -> URL? {
    guard !site.isEmpty, let key = r.fields["key"], !key.isEmpty else { return nil }
    guard jiraIsReleaseRow(r) else { return URL(string: site + "/browse/" + key) }
    let proj = r.fields["project"] ?? "", name = r.fields["release"] ?? r.title
    let id = (r.fields["versionId"] ?? "").isEmpty
        ? JiraDirectory.load().versions.first { $0.project == proj && $0.name == name }?.id ?? ""
        : r.fields["versionId"] ?? ""
    if !id.isEmpty {
        let tab = site.contains(".atlassian.net") ? "/tab/release-report-all-issues" : ""
        return URL(string: "\(site)/projects/\(proj)/versions/\(id)\(tab)")
    }
    var c = URLComponents(string: site + "/issues/")
    c?.queryItems = [URLQueryItem(name: "jql", value: "project = \"\(proj)\" AND fixVersion = \"\(name)\"")]
    return c?.url
}

func iconForApp(_ app: AppInfo) -> NSImage {
    if let rule = iconRules.first(where: { $0.app == app.name }) {
        let title = app.windowTitle?.lowercased() ?? ""
        for (match, img) in rule.titleMatches where title.contains(match) {
            return img
        }
        if let d = rule.defaultIcon { return d }
    }
    var url: URL?
    if let bid = app.bundleID {
        url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bid)
    }
    if url == nil {
        for dir in settings.appDirs where FileManager.default.fileExists(
            atPath: dir + "/" + app.name + ".app") {
            url = URL(fileURLWithPath: dir + "/" + app.name + ".app")
            break
        }
    }
    if let url { return NSWorkspace.shared.icon(forFile: url.path) }
    return missingIcon
}

struct WorkspaceRow: PopupRow {
    let title: String
    let icons: [NSImage]
    let trailing: String?
    let focused: Bool
    var unread = ""
    var match: String?

    init(ws: WorkspaceInfo, iconCache: inout [String: NSImage]) {
        title = ws.id
        focused = ws.focused
        var imgs: [NSImage] = []
        for app in ws.apps.prefix(rowMaxIcons) {
            let key = (app.bundleID ?? app.name) + "|" + (app.windowTitle ?? "")
            if let cached = iconCache[key] {
                imgs.append(cached)
            } else {
                let img = iconForApp(app)
                iconCache[key] = img
                imgs.append(img)
            }
        }
        icons = imgs
        let extra = ws.apps.count - rowMaxIcons
        trailing = extra > 0 ? "+\(extra)" : nil
    }
}

struct CommandRow: PopupRow {
    let title: String
    let command: CommandSpec
    init(_ c: CommandSpec) { title = "> \(c.label ?? c.name)"; command = c }
}

struct SplitRow: PopupRow {
    let command: CommandSpec?
    let workspace: WorkspaceRow?
    var title: String { command.map { $0.label ?? $0.name } ?? workspace?.title ?? "" }
}

struct FieldRow: PopupRow {
    let title: String
    let content: String?
    let trailing: String?
    let detail: String?
    let body: String?
    let searchText: String
    let fields: [String: String]
    var starred: Bool? = nil

    var loadMore: Bool { fields["__loadmore"] != nil }
    var groupHeader: Bool { fields["__group"] != nil }
    var synthetic: Bool { loadMore || groupHeader }
    func cellText(_ field: String) -> String? { fields[field].map(compactTimestamp) }
}

private let isoParsers: [DateFormatter] = [
    "yyyy-MM-dd'T'HH:mm:ss.SSSZ", "yyyy-MM-dd'T'HH:mm:ssZ",
    "yyyy-MM-dd'T'HH:mm:ss.SSSXXXXX", "yyyy-MM-dd'T'HH:mm:ssXXXXX",
].map { fmt in
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.dateFormat = fmt
    return f
}
private let compactStampFormatter: DateFormatter = {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.dateFormat = "yyyy-MM-dd HH:mm"
    return f
}()
private let compactStampCache: NSCache<NSString, NSString> = {
    let c = NSCache<NSString, NSString>()
    c.countLimit = 50_000
    return c
}()
func compactTimestamp(_ s: String) -> String {
    let u = Array(s.utf8)
    guard u.count >= 19, u.count <= 35, u[4] == 45, u[7] == 45, u[10] == 84, u[13] == 58
    else { return s }
    if let hit = compactStampCache.object(forKey: s as NSString) { return hit as String }
    var out = s
    for p in isoParsers {
        if let d = p.date(from: s) { out = compactStampFormatter.string(from: d); break }
    }
    compactStampCache.setObject(out as NSString, forKey: s as NSString)
    return out
}

final class VoiceRecorder {
    enum State: Int { case idle = 0, recording = 1, paused = 2, transcribing = 3 }
    private(set) var state: State = .idle
    private(set) var elapsed: TimeInterval = 0
    var onStateChange: ((State) -> Void)?
    var onPartial: ((String) -> Void)?
    var onBatch: ((String) -> Void)?
    var onError: ((String) -> Void)?
    var onLevel: ((Float) -> Void)?

    private let engine = AVAudioEngine()
    private var recognizer: SFSpeechRecognizer?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var finalizing: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var ticker: Timer?
    private var batchTimer: Timer?
    private var teardownTimer: DispatchWorkItem?
    private var lastBatchAt: TimeInterval = 0
    private var lastLevelSample: TimeInterval = 0
    private let batchInterval: TimeInterval = 300
    private let stopTeardownTimeout: TimeInterval = 3

    private func ensureAuthorized() -> Bool {
        let mic = AVCaptureDevice.authorizationStatus(for: .audio)
        let speech = SFSpeechRecognizer.authorizationStatus()
        if mic == .authorized && speech == .authorized { return true }
        if Bundle.main.bundleIdentifier != nil,
           mic == .notDetermined || speech == .notDetermined {
            if mic == .notDetermined {
                AVCaptureDevice.requestAccess(for: .audio) { _ in }
            }
            if speech == .notDetermined {
                SFSpeechRecognizer.requestAuthorization { _ in }
            }
            onError?("grant the microphone/speech prompt that just appeared, then press record again")
            return false
        }
        let denied = mic == .denied || speech == .denied
            || mic == .restricted || speech == .restricted
        onError?(denied
            ? "microphone/speech access is blocked for this binary — run bin/grant-permissions.sh (or add it in System Settings > Privacy & Security > Microphone AND Speech Recognition), then press record again"
            : "microphone/speech permission not granted yet — run bin/grant-permissions.sh (or add this binary in System Settings > Privacy & Security > Microphone AND Speech Recognition), then press record again")
        return false
    }

    func start() {
        guard state == .idle, ensureAuthorized() else { return }
        guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: settings.voiceLocale)),
              recognizer.isAvailable else {
            onError?("speech recognizer unavailable for locale '\(settings.voiceLocale)'")
            return
        }
        self.recognizer = recognizer
        let input = engine.inputNode
        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 1024,
                         format: input.outputFormat(forBus: 0)) { [weak self] buffer, _ in
            self?.appendBuffer(buffer)
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            resetSession()
            onError?("recording failed: \(error.localizedDescription)")
            return
        }
        elapsed = 0
        lastBatchAt = 0
        startBatch()
        state = .recording
        startTicker()
        startBatchTimer()
        onStateChange?(state)
    }

    private func startBatch() {
        guard let recognizer else { return }
        let r = SFSpeechAudioBufferRecognitionRequest()
        r.shouldReportPartialResults = true
        request = r
        task = recognizer.recognitionTask(with: r) { [weak self] result, error in
            DispatchQueue.main.async {
                guard let self else { return }
                if let result {
                    let text = result.bestTranscription.formattedString
                    if result.isFinal {
                        let isOurs = self.finalizing === r || self.request === r
                        if self.finalizing === r { self.finalizing = nil }
                        let wasCurrent = self.request === r
                        if wasCurrent { self.request = nil; self.task = nil }
                        if isOurs { self.onBatch?(text) }
                        if wasCurrent && self.state == .recording {
                            self.startBatch()
                            self.lastBatchAt = self.elapsed
                        }
                    } else {
                        if self.request === r {
                            self.onPartial?(text)
                        }
                    }
                } else if let error {
                    let wasCurrent = self.request === r
                    if self.finalizing === r { self.finalizing = nil }
                    if wasCurrent { self.request = nil; self.task = nil }
                    if self.state == .recording {
                        if wasCurrent {
                            self.startBatch()
                            self.lastBatchAt = self.elapsed
                        }
                    } else if self.state == .paused {
                    } else {
                        let s = (error as NSError).localizedDescription.lowercased()
                        if s.contains("no speech") || s.contains("canceled") {
                            self.onBatch?("")
                        } else {
                            self.onError?("transcription failed: \(error.localizedDescription)")
                        }
                    }
                }
            }
        }
    }

    private func finalizeBatch() {
        if let r = request {
            r.endAudio()
            finalizing = r
            request = nil
        }
    }

    private func appendBuffer(_ buffer: AVAudioPCMBuffer) {
        request?.append(buffer)
        let now = ProcessInfo.processInfo.systemUptime
        guard state == .recording, now - lastLevelSample > 0.08 else { return }
        lastLevelSample = now
        guard let ch = buffer.floatChannelData?[0] else { return }
        let n = Int(buffer.frameLength)
        guard n > 0 else { return }
        var sum: Float = 0
        for i in 0..<n { sum += ch[i] * ch[i] }
        let rms = sqrt(sum / Float(n))
        let lvl = Float(min(1, max(0, rms * 4)))
        DispatchQueue.main.async { [weak self] in
            self?.onLevel?(lvl)
        }
    }

    func pause() {
        guard state == .recording else { return }
        finalizeBatch()
        state = .paused
        onStateChange?(state)
    }

    func resume() {
        guard state == .paused else { return }
        startBatch()
        lastBatchAt = elapsed
        state = .recording
        onStateChange?(state)
    }

    func stop() {
        guard state == .recording || state == .paused else { return }
        finalizeBatch()
        ticker?.invalidate()
        ticker = nil
        batchTimer?.invalidate()
        batchTimer = nil
        state = .transcribing
        onStateChange?(state)
        teardownTimer?.cancel()
        let item = DispatchWorkItem { [self] in
            self.resetSession()
        }
        teardownTimer = item
        DispatchQueue.main.asyncAfter(deadline: .now() + stopTeardownTimeout,
                                      execute: item)
    }

    func resetSession() {
        teardownTimer?.cancel()
        teardownTimer = nil
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        request = nil
        finalizing = nil
        task = nil
        state = .idle
        onStateChange?(state)
    }

    private func startTicker() {
        ticker?.invalidate()
        ticker = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            guard let self, self.state != .idle else { return }
            if self.state == .recording {
                self.elapsed += 0.1
            }
            if self.state == .paused {
                self.onLevel?(0.03)
            }
        }
    }

    private func startBatchTimer() {
        batchTimer?.invalidate()
        batchTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            guard let self, self.state == .recording,
                  self.elapsed - self.lastBatchAt >= self.batchInterval else { return }
            self.finalizeBatch()
            self.startBatch()
            self.lastBatchAt = self.elapsed
        }
    }
}

final class SwitcherController: NSObject {
    let popup: PopupWindow
    let commandRunner: CommandRunner?
    var workspaces: [WorkspaceInfo] = []
    var commands: [CommandSpec] = []
    var splitPane = 0
    var stripSig = ""
    private var stripTimer: Timer?
    private var splitQuery = ""
    var status = SwitcherStatus()
    var savedWID: String?
    var savedPID: pid_t?
    var pathsWindow: PathsWindow?
    var noteFindWindow: NoteFindWindow?
    var noteGrepWindow: NoteFindWindow?
    var sidebarJumpWindow: SidebarJumpWindow?
    var clipboardPaths: ClipboardPaths?
    var pathsSeedObserver: NSObjectProtocol?
    lazy var screenshot: ScreenshotController = {
        let s = ScreenshotController()
        s.log = { [weak self] in self?.log($0) }
        s.onSaved = {
            RecentFiles.shared.ownChange(from: nil, to: $0)
            PathShelf.shared.add([$0], why: .screenshot)
        }
        s.onOwnPasteboardWrite = { [weak self] in self?.clipboardPaths?.ownWrite() }
        return s
    }()
    private var iconCache: [String: NSImage] = [:]
    var subWindows: [PopupWindow] = []
    var jiraShowTab: ((String) -> Void)?
    var pendingJiraTab: String?
    var detailRow: FieldRow?
    var pendingReleaseTab: String?
    private var lastOtherAppClick: Date?
    private var appActivations = 0
    private var globalClickMonitor: Any?
    private var prettyFormatWorkItem: DispatchWorkItem?
    private weak var pickerWindow: PopupWindow?
    private var pickerRole: PopupWindow.ThemeRole?
    private var pickerSection = ""
    private var pickerHue: NSColor = .clear
    private var pickerTransparency: CGFloat = 0.0
    private var pickerHexLabel: NSTextField?
    private var pickerTransparencyLabel: NSTextField?
    private var pickerOriginal: NSColor?
    private var pickerCommitted = false
    private var pickerSawVisible = false
    private var pickerWatchdog: Timer?
    private var pickerPanelObserver: Any?

    override init() {
        var config = PopupConfig(name: settings.switcherWindowName)
        config.colors = windowColors()
        config.width = settings.switcherWidth
        config.enableResize = true
        config.clickToSelect = true
        config.dynamicHeight = true
        config.toolPanel = true
        popup = PopupWindow(config: config)
        commandRunner = CommandRunner()
        super.init()
        commands = loadCommands()
        PopupWindow.keyInterceptor = { [weak self] e, w in self?.slot.prefixKey(e, in: w) ?? false }
        PaneNav.shared.provider = { [weak self] w in
            guard let self, let cur = self.slot.current, let m = self.slotMember(cur),
                  m.slotWindow === w else { return nil }
            return m as? PaneProvider
        }

        popup.onFilter = { [weak self] query in
            self?.filter(query) ?? []
        }
        popup.onAccept = { [weak self] row in
            self?.accept(row)
        }
        popup.onEscape = { [weak self] in
            self?.handleEscape()
        }
        popup.onShow = { [weak self] in
            guard let self else { return }
            self.popup.initialQuery = ""
            self.popup.selection = 0
            self.splitPane = 0
            (self.savedWID, self.savedPID) = readFocusFile()
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let fresh = gatherWorkspaces()
                DispatchQueue.main.async { [weak self] in
                    guard let self, !fresh.isEmpty else { return }
                    self.workspaces = fresh
                    self.refreshRows()
                }
            }
            let notify = assetDir + "/notify/notify_poll.py"
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let st = SwitcherStatus.gather(notifyScript: notify)
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    self.status = st
                    self.refreshRows()
                }
            }
        }
        popup.onKeyPreview = { [weak self] code, mods in
            self?.switcherKey(code, mods) ?? false
        }
        popup.onRowClick = { [weak self] index in
            self?.clickRow(index)
        }
        popup.onHide = { [weak self] restore in
            self?.restoreFocus(restore)
        }
        popup.onDrawRow = { [weak self] rect, row, selected in
            self?.drawRow(rect, row, selected)
        }
    }

    func start() {
        popup.start()
        startCommandServer()
        startFocusBridge()
        startScreenshotYield()
        startStrayGuard()
        requestUnreadAccess()
    }

    private func requestUnreadAccess() {
        guard !AXIsProcessTrusted(),
              tri(configSectionValue("notifications", "enabled")) ?? false else { return }
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        _ = AXIsProcessTrustedWithOptions([key: true] as CFDictionary)
        log("unread counts: asked for Accessibility (System Settings ▸ Privacy & Security ▸ Accessibility)")
    }

    private var strayGuardWork: DispatchWorkItem?
    private func startStrayGuard() {
        let run: (String) -> Void = { [weak self] sender in
            self?.strayGuardWork?.cancel()
            let work = DispatchWorkItem {
                let script = assetDir + "/bin/no_stray_workspaces.sh"
                guard FileManager.default.isExecutableFile(atPath: script) else { return }
                var env = ProcessInfo.processInfo.environment
                env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:" + (env["PATH"] ?? "")
                env["SENDER"] = sender
                _ = try? runProcess(script, [], env: env)
            }
            self?.strayGuardWork = work
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 0.5, execute: work)
        }
        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { _ in run("display_change") }
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { _ in run("system_woke") }
    }

    private var yieldedWindows: [NSWindow] = []
    private func startScreenshotYield() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] note in
            guard let self else { return }
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            let id = app?.bundleIdentifier ?? ""
            if settings.screenshotApps.contains(id) {
                guard self.yieldedWindows.isEmpty else { return }
                self.yieldedWindows = NSApp.windows.filter {
                    $0.isVisible && $0.alphaValue > 0 && $0.delegate is PopupWindow
                }
                for w in self.yieldedWindows {
                    w.alphaValue = 0
                    w.ignoresMouseEvents = true
                }
                if !self.yieldedWindows.isEmpty { self.log("screenshot tool \(id) active — popups yield") }
            } else if !self.yieldedWindows.isEmpty {
                for w in self.yieldedWindows {
                    w.alphaValue = 1
                    w.ignoresMouseEvents = false
                }
                self.yieldedWindows = []
            }
        }
    }

    private var bridgeMtime: (Int, Int)?
    var viewSwitcher: ViewSwitcherPanel?
    var terminalPanel: TerminalPanel?
    private var bridgeSource: DispatchSourceFileSystemObject?
    private func startFocusBridge() {
        if globalClickMonitor == nil,
           let m = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown, handler: { [weak self] event in
            guard let self else { return }
            let p = NSEvent.mouseLocation
            let inOurWindow = self.subWindows.contains {
                $0.isShown && $0.nativeWindow.frame.contains(p)
            }
            if !inOurWindow {
                self.lastOtherAppClick = Date()
            }
        }) {
            globalClickMonitor = m
        }
        NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main
        ) { [weak self] note in
            guard let self, !NSApp.isActive, let w = note.object as? NSWindow, w.isVisible,
                  Self.sharedViews.contains(where: { self.slotMember($0)?.slotWindow === w })
                    || self.subWindows.contains(where: { $0.nativeWindow === w && $0.isShown && !$0.config.toolPanel })
            else { return }
            if let t = self.lastOtherAppClick, Date().timeIntervalSince(t) < 1.0 { return }
            NSApp.activate(ignoringOtherApps: true)
            w.orderFrontRegardless()
            self.log("our window became key while inactive (aerospace focus) — self-activated")
        }
        NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.appActivations += 1 }
        watchFocusBridge()
        var tick = 0
        stripTimer = Timer.scheduledTimer(withTimeInterval: 6, repeats: true) { [weak self] _ in
            guard let self, self.slot.isVisible else { return }
            tick += 1
            if tick % 5 == 0 {
                let notify = assetDir + "/notify/notify_poll.py"
                DispatchQueue.global(qos: .utility).async { [weak self] in
                    let st = SwitcherStatus.gather(notifyScript: notify)
                    DispatchQueue.main.async { self?.status = st }
                }
            }
            self.refreshWorkspaceStrip()
        }
    }

    private func watchFocusBridge() {
        let path = popupTmpDir() + settings.focusBridgeName
        if !FileManager.default.fileExists(atPath: path) {
            FileManager.default.createFile(atPath: path, contents: nil)
        }
        let fd = open(path, O_EVTONLY)
        guard fd >= 0 else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.watchFocusBridge() }
            return
        }
        let src = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd, eventMask: [.write, .extend, .attrib, .delete, .rename], queue: .main)
        src.setEventHandler { [weak self, weak src] in
            guard let self, let src else { return }
            if !src.data.isDisjoint(with: [.delete, .rename]) {
                src.cancel()
                self.bridgeSource = nil
                DispatchQueue.main.async { [weak self] in
                    self?.watchFocusBridge()
                    self?.focusBridgeChanged(path)
                }
                return
            }
            self.focusBridgeChanged(path)
        }
        src.setCancelHandler { close(fd) }
        bridgeSource = src
        src.resume()
    }

    private func focusBridgeChanged(_ path: String) {
        if slot.isVisible { refreshWorkspaceStrip() }
        var st = stat()
        guard stat(path, &st) == 0 else { return }
        let mt = (Int(st.st_mtimespec.tv_sec), Int(st.st_mtimespec.tv_nsec))
        if let prev = bridgeMtime, prev.0 == mt.0, prev.1 == mt.1 { return }
        bridgeMtime = mt
        guard let raw = try? String(contentsOfFile: path, encoding: .utf8),
              let id = Int(raw.trimmingCharacters(in: .whitespacesAndNewlines)),
              let w = subWindows.first(where: {
                  $0.isShown && $0.nativeWindow.windowNumber == id && !$0.config.toolPanel
              }) else { return }
        let age = Date().timeIntervalSince1970 - (Double(mt.0) + Double(mt.1) / 1_000_000_000)
        let clickedAway = lastOtherAppClick.map { Date().timeIntervalSince($0) < 1.0 } ?? false
        guard !clickedAway, age < 1.0 else { return }
        NSApp.activate(ignoringOtherApps: true)
        w.nativeWindow.orderFrontRegardless()
        w.nativeWindow.makeKeyAndOrderFront(nil)
        log("aerospace focused our window \(id) — self-activated")
    }

    lazy var slot = SharedWindow(controller: self)
    var pendingSlotFrame: NSRect?
    var slotPrewarming = false
    private var prewarmGen = 0

    func prewarmSlot(after delay: Double = 1.0) {
        guard settings.sharedWindow, settings.preload else { return }
        prewarmGen += 1
        let gen = prewarmGen
        var views: [SlotView] = [.notes]
        if aiEnabled() { views.append(.ai) }
        views.append(.files)
        if jiraEnabledInConfig() { views.append(.jira) }
        if confluenceEnabled() { views.append(.confluence) }
        if compareEnabled() { views.append(.compare) }
        func step(_ rest: ArraySlice<SlotView>, _ wait: Double) {
            guard let v = rest.first else { return }
            DispatchQueue.main.asyncAfter(deadline: .now() + wait) { [weak self] in
                guard let self, gen == self.prewarmGen else { return }
                if self.slotMember(v) == nil {
                    let t0 = DispatchTime.now().uptimeNanoseconds
                    self.slotPrewarming = true
                    let ok = self.ensureSlotMember(v, frame: self.slot.currentFrame())
                    self.slotPrewarming = false
                    if ok { self.slot.prepare(v) }
                    let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
                    self.log(String(format: "preload %@: %@%.0f ms", v.rawValue, ok ? "" : "not available, ", ms))
                }
                step(rest.dropFirst(), 0.3)
            }
        }
        step(views[...], delay)
    }
    var currentOutputName: String?
    private var filesCommand: CommandSpec? { commands.first { $0.kind == .files } }

    func slotMember(_ v: SlotView) -> SlotMember? {
        switch v {
        case .notes: return noteWindow
        case .files: return subWindows.first { $0.config.name == filesCommand?.windowName }
        case .output: return subWindows.first { $0.config.name == currentOutputName }
        case .jira: return subWindows.first { $0.config.name == "jira" }
        case .detail: return subWindows.first { $0.config.name == settings.detailWindowName }
        case .releases: return subWindows.first { $0.config.name == jiraReleasesWindow }
        case .config: return JiraDashboardWindow.current
        case .confluence: return ConfluenceWindow.current
        case .ai: return AIWindow.current
        case .compare: return CompareWindow.current
        case .compareText: return CompareWindow.sub
        }
    }

    func slotSection(_ v: SlotView) -> String? {
        switch v {
        case .notes: return commands.first { $0.kind == .note }?.name
        case .files: return filesCommand?.name
        case .jira, .detail, .releases, .config: return "jira"
        case .confluence: return "confluence"
        case .ai: return "ai"
        case .compare, .compareText: return "compare"
        case .output: return currentOutputName.flatMap { n in commands.first { $0.windowName == n }?.name }
        }
    }

    func escHideCount(_ v: SlotView, lines: [String]? = nil) -> Int {
        guard let sec = slotSection(v),
              let lines = lines ?? readConfigText().map(configLines) else { return settings.escClose }
        let entries = configSectionEntries(lines, sec)
        let own = (entries.first { $0.key == "esc-close" } ?? entries.first { $0.key == "vim-esc-close" })?.value
        return max(0, own.flatMap { Int($0.trimmingCharacters(in: .whitespaces)) } ?? settings.escClose)
    }

    func setEscHides(_ v: SlotView, _ on: Bool) {
        guard let sec = slotSection(v) else { return }
        let value: String? = on ? "1" : settings.escClose == 0 ? nil : "0"
        saveConfigValues(section: sec, [("esc-close", value), ("vim-esc-close", nil)])
        let n = escHideCount(v)
        if let i = commands.firstIndex(where: { $0.name == sec }) { commands[i].escClose = value.flatMap(Int.init) }
        if v == .notes { noteWindow?.config.escCloseCount = n }
        log("[\(sec)] esc-close = \(value ?? "(default \(settings.escClose))") — Esc \(n > 0 ? "hides" : "never hides") the window")
    }

    func escHidesMenuItem(_ v: SlotView) -> NSMenuItem {
        let on = escHideCount(v) > 0
        let item = menuItem("Esc Hides Window", state: on) { [weak self] in self?.setEscHides(v, !on) }
        item.toolTip = v == .notes
            ? "Esc in Normal mode (vim) / the editor hides the window. Off: hide with ✕, Cmd+W or the hotkey"
            : "Esc hides the window once there is nothing left to clear or step back from. Off: ✕, Cmd+W or the hotkey"
        return item
    }

    func slotView(of w: PopupWindow) -> SlotView? {
        guard settings.sharedWindow else { return nil }
        let n = w.config.name
        if n == commands.first(where: { $0.kind == .note })?.windowName { return .notes }
        if n == filesCommand?.windowName { return .files }
        if commands.contains(where: { $0.kind == .output && !$0.panel && $0.windowName == n }) { return .output }
        if n == "jira" { return .jira }
        if n == settings.detailWindowName { return .detail }
        if n == jiraReleasesWindow { return .releases }
        return nil
    }

    func ensureSlotMember(_ v: SlotView, frame: NSRect) -> Bool {
        if slotMember(v) != nil { return true }
        switch v {
        case .notes:
            guard let cmd = commands.first(where: { $0.kind == .note }) else { return false }
            pendingSlotFrame = frame
            openNoteWindow(cmd, restoreWID: nil, restorePID: nil)
        case .jira:
            guard let cmd = commands.first(where: { $0.name == "jira" }) else { return false }
            pendingSlotFrame = frame
            openListWindow(cmd, restoreWID: nil, restorePID: nil)
        case .files:
            guard let cmd = filesCommand else { return false }
            pendingSlotFrame = frame
            openFilesWindow(cmd, restoreWID: nil, restorePID: nil)
        case .confluence:
            guard confluenceEnabled() else { return false }
            ConfluenceWindow.create(controller: self, frame: frame)
        case .ai:
            guard aiEnabled() else { return false }
            AIWindow.create(controller: self, frame: frame)
        case .compare:
            guard compareEnabled() else { return false }
            CompareWindow.create(controller: self, frame: frame)
        default:
            return false
        }
        pendingSlotFrame = nil
        return slotMember(v) != nil
    }

    func slotShowFiles(hotkey: Bool = false, userInIt: Bool? = nil) {
        let wasShown = slot.current == .files && slot.isVisible
        if hotkey { slot.hotkey(.files, userInIt: userInIt) } else { slot.open(.files) }
        if !wasShown, slot.current == .files, slot.isVisible, filesCommand?.startRecent ?? true,
           let w = slotMember(.files) as? PopupWindow {
            w.fileBrowser?.showRecent()
        }
    }

    func placeSlotWindow(_ w: PopupWindow) {
        guard settings.sharedWindow, slotView(of: w) != nil else { return }
        w.initialFrame = pendingSlotFrame ?? slot.currentFrame()
        pendingSlotFrame = nil
    }

    func restoreFocus(wid: String?, pid: pid_t?) {
        if let pid, pid != getpid() {
            NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateAllWindows])
        }
        if let wid {
            DispatchQueue.global(qos: .userInitiated).async {
                _ = aerospaceCall(["focus", "--window-id", wid])
            }
        }
    }

    func closeSlotWindow(_ w: PopupWindow) {
        if w.isShown { w.hide(restore: false) } else { w.onHide?(false) }
        w.nativeWindow.orderOut(nil)
    }

    func showNotes() {
        if popup.isShown {
            popup.hide(restore: false)
        }
        if settings.sharedWindow {
            if !slot.isVisible { (savedWID, savedPID) = readFocusFile() }
            slot.open(.notes)
            return
        }
        (savedWID, savedPID) = readFocusFile()
        guard let cmd = commands.first(where: { $0.kind == .note }) else {
            log("notes: no note command configured in \(commandsConfName)")
            return
        }
        if let w = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            focusSubWindow(w)
            return
        }
        openNoteWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
    }

    func openNoteFile(_ path: String) {
        let p = (path as NSString).standardizingPath
        guard FileManager.default.fileExists(atPath: p) else {
            log("openNoteFile: \(p) does not exist")
            return
        }
        if popup.isShown {
            popup.hide(restore: false)
        }
        (savedWID, savedPID) = readFocusFile()
        guard let cmd = commands.first(where: { $0.kind == .note }) else {
            log("openNoteFile: no note command configured in \(commandsConfName)")
            return
        }
        if settings.sharedWindow {
            slot.open(.notes)
        } else if let w = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            focusSubWindow(w)
        } else {
            openNoteWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
        }
        if let w = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            w.onOpenExternalPath?(p)
        }
    }

    func showCommand(_ name: String) {
        let name = name == "voice" ? "notes" : name
        if popup.isShown {
            popup.hide(restore: false)
        }
        (savedWID, savedPID) = readFocusFile()
        guard let cmd = commands.first(where: { $0.name == name }) else {
            log("launch: no command named '\(name)' in \(commandsConfName)")
            return
        }
        if isToolPanel(cmd) {
            openTool(cmd)
            return
        }
        if settings.sharedWindow && (cmd.kind == .note || cmd.name == "jira") {
            slot.open(cmd.kind == .note ? .notes : .jira)
            return
        }
        if settings.sharedWindow && cmd.kind == .files {
            slotShowFiles()
            return
        }
        switch cmd.kind {
        case .note:
            if let existing = subWindows.first(where: { $0.config.name == cmd.windowName }) {
                focusSubWindow(existing)
                return
            }
            openNoteWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
        case .list:
            focusExistingOrOpen(named: cmd.windowName) {
                openListWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
            }
        case .files:
            if let existing = subWindows.first(where: { $0.config.name == cmd.windowName }) {
                if cmd.startRecent { existing.fileBrowser?.showRecent() }
                focusSubWindow(existing)
                return
            }
            openFilesWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
        case .output:
            openOutputWindow(cmd)
        case .shell:
            log("launch: '\(name)' is a shell command, nothing to open")
        }
    }

    private func focusExistingOrOpen(named name: String, open: () -> Void) {
        popup.hide(restore: false)
        if let existing = subWindows.first(where: { $0.config.name == name }) {
            focusSubWindow(existing)
            return
        }
        open()
    }

    private func focusSubWindow(_ w: PopupWindow) {
        if !w.isShown {
            w.showPersistent()
        } else {
            let win = w.nativeWindow
            win.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        log("focused existing '\(w.config.name)' window")
    }

    static let hotkeyModes: Set<String> = ["window", "notes", "voice", "jira", "files", "confluence", "ai", "compare"]

    struct HotkeyPrep {
        var log = ""
        var workspace = ""
        var screen: Int?
        var cacheCleared = false
    }

    static func hotkeyPrep() -> HotkeyPrep {
        let t0 = DispatchTime.now().uptimeNanoseconds
        var focused = "", rows = "", ws = ""
        let g = DispatchGroup()
        DispatchQueue.global(qos: .userInteractive).async(group: g) {
            focused = aerospaceCall(["list-windows", "--focused", "--format", "%{window-id} %{app-pid}"])
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
        var cleared = false
        DispatchQueue.global(qos: .userInteractive).async(group: g) {
            cleared = aerospaceSocket(["eval", "true"]) != nil
        }
        rows = aerospaceCall(["list-windows", "--all", "--format",
                              "%{window-id}|%{app-pid}|%{workspace}|%{workspace-is-focused}|%{monitor-appkit-nsscreen-screens-id}|%{window-title}"])
        let table = rows.split(separator: "\n").map {
            $0.split(separator: "|", maxSplits: 5, omittingEmptySubsequences: false).map(String.init)
        }.filter { $0.count == 6 }
        if let f = table.first(where: { $0[3] == "true" }) {
            ws = f[2] + "|" + f[4]
        } else {
            ws = aerospaceCall(["list-workspaces", "--focused", "--format",
                                "%{workspace}|%{monitor-appkit-nsscreen-screens-id}"])
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
        g.wait()
        if focused.split(separator: " ").count == 2 {
            try? focused.write(toFile: settings.focusFilePath, atomically: true, encoding: .utf8)
        } else {
            try? FileManager.default.removeItem(atPath: settings.focusFilePath)
        }
        var prep = HotkeyPrep()
        prep.cacheCleared = cleared
        let wsParts = ws.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        let cur = wsParts.first ?? ""
        prep.workspace = cur
        prep.screen = wsParts.count > 1 ? Int(wsParts[1]) : nil
        let me = String(getpid())
        var moved: [String] = []
        for f in table {
            guard f[1] == me, !cur.isEmpty, f[2] != cur, f[2] != "N", f[5] != settings.switcherWindowName else { continue }
            _ = aerospaceCall(["move-node-to-workspace", "--window-id", f[0], cur])
            moved.append(f[0])
        }
        let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
        prep.log = String(format: "prep %.1f ms (ws %@, screen %@%@)", ms, cur, prep.screen.map(String.init) ?? "?",
                          moved.isEmpty ? "" : ", moved " + moved.joined(separator: ","))
        return prep
    }

    func applyHotkeyPrep(_ prep: HotkeyPrep) {
        slot.targetScreen = prep.screen
        slot.aerospaceCacheCleared = prep.cacheCleared
    }

    func openSlotHere(_ show: @escaping (SwitcherController) -> Void) {
        guard settings.sharedWindow else { show(self); return }
        DispatchQueue.global(qos: .userInteractive).async { [weak self] in
            let prep = Self.hotkeyPrep()
            DispatchQueue.main.async {
                guard let self else { return }
                self.applyHotkeyPrep(prep)
                show(self)
                self.slot.aerospaceCacheCleared = false
                self.log("palette view: \(prep.log)")
            }
        }
    }

    func testQuery(_ q: String) -> String {
        if q.hasPrefix("do:") {
            let a = String(q.dropFirst(3))
            switch a {
            case "cycle": slot.cycle(1)
            case "cycle-back": slot.cycle(-1)
            case "hide": slot.hide("test")
            case "back": slot.back()
            case "home": slot.home()
            case "toggle": slot.toggle()
            case "toggle-terminal": noteWindow?.toggleTerminalDrawer()
            case "term": toggleTerminalPanel()
            case "notes-find": showNoteFind()
            case "notes-grep": showNoteFind(grep: true)
            case "jira-jump":
                if let w = slotMember(.jira) as? PopupWindow { showSidebarJump(w) }
            case "switcher": showViewSwitcher()
            case "switcher-hide": viewSwitcher?.window.hide(restore: false)
            case "reset-size": noteWindow?.resetToDefaultSize()
            case _ where a.hasPrefix("open:"):
                guard let v = SlotView(rawValue: String(a.dropFirst(5))) else {
                    return "{\"error\":\"unknown view\"}"
                }
                slot.open(v)
            case _ where a.hasPrefix("rebuild-card:"):
                guard let v = SlotView(rawValue: String(a.dropFirst(13))),
                      [.confluence, .ai, .compare, .compareText].contains(v) else {
                    return "{\"error\":\"not a card view\"}"
                }
                rebuildCard(v)
            case _ where a.hasPrefix("board:"):
                (slotMember(.jira) as? PopupWindow)?.onTestAction?(String(a.dropFirst(6)))
            case _ where a.hasPrefix("paths:"):
                let arg = String(a.dropFirst(6))
                switch arg {
                case "show":
                    guard let cmd = pathsCommand else { return "{\"error\":\"[paths] not enabled\"}" }
                    showPaths(cmd)
                case "hide": pathsWindow?.hide()
                case "return": pathsWindow?.testReturn()
                case _ where arg.hasPrefix("select:"):
                    pathsWindow?.testSelect(Int(arg.dropFirst(7)) ?? 0)
                default: return "{\"error\":\"paths:show|hide|return|select:N\"}"
                }
            case _ where a.hasPrefix("tool:"):
                let n = String(a.dropFirst(5))
                guard let cmd = commands.first(where: { $0.name == n }), isToolPanel(cmd) else {
                    return "{\"error\":\"not a tool panel: \(n)\"}"
                }
                openTool(cmd)
            case _ where a.hasPrefix("compare:"):
                if let err = compareTestDo(String(a.dropFirst(8))) {
                    return "{\"error\":\"\(err)\"}"
                }
            case _ where a.hasPrefix("screenshot:"):
                if let err = screenshot.testDo(String(a.dropFirst(11))) {
                    return "{\"error\":\"\(err)\"}"
                }
            case _ where a.hasPrefix("tool-close:"):
                let n = String(a.dropFirst(11))
                if n == "screenshot" { screenshot.session?.finish(.abort) }
                if n == "paths" { pathsWindow?.hide() }
                else { subWindows.first { $0.config.name == n && $0.config.toolPanel }?.hide(restore: false) }
            case _ where a.hasPrefix("esc-hides:"):
                let parts = a.split(separator: ":").map(String.init)
                guard parts.count == 3, let v = SlotView(rawValue: parts[1]), ["on", "off"].contains(parts[2]) else {
                    return "{\"error\":\"esc-hides:VIEW:on|off\"}"
                }
                setEscHides(v, parts[2] == "on")
            case _ where a.hasPrefix("pane:"):
                guard let cur = slot.current, let w = slotMember(cur)?.slotWindow else {
                    return "{\"error\":\"no view shown\"}"
                }
                let arg = String(a.dropFirst(5))
                if arg.hasPrefix("focus:") {
                    guard PaneNav.shared.focus(String(arg.dropFirst(6)), in: w) else {
                        return "{\"error\":\"no such pane\"}"
                    }
                } else if let d = PaneDir(rawValue: arg) {
                    _ = PaneNav.shared.move(d, in: w)
                } else {
                    return "{\"error\":\"pane:h|j|k|l|focus:ID\"}"
                }
            case _ where a.hasPrefix("key:"):
                guard let cur = slot.current, let w = slotMember(cur)?.slotWindow,
                      let e = CompareWindow.keyEvent(String(a.dropFirst(4)), window: w) else {
                    return "{\"error\":\"no view shown / bad key\"}"
                }
                NSApp.postEvent(e, atStart: false)
            case _ where a.hasPrefix("header-style:"):
                guard let st = HeaderStyle(rawValue: String(a.dropFirst(13))) else {
                    return "{\"error\":\"unknown header style\"}"
                }
                HeaderStyle.current = st
            default: return "{\"error\":\"unknown action \(a)\"}"
            }
        }
        var views: [String: Any] = [:]
        for v in [SlotView.notes, .files, .jira, .detail, .releases, .config, .output, .confluence, .ai, .compare, .compareText] {
            guard let m = slotMember(v) else { continue }
            if let p = m as? PopupWindow {
                var st = p.testState
                st["wid"] = m.slotWindow.windowNumber
                views[v.rawValue] = st
            } else {
                let f = m.slotWindow.frame
                views[v.rawValue] = ["shown": m.slotShown, "key": m.slotWindow.isKeyWindow, "wid": m.slotWindow.windowNumber,
                                     "frame": [f.origin.x, f.origin.y, f.width, f.height].map { Int($0.rounded()) }]
            }
        }
        let state: [String: Any] = [
            "view": slot.current?.rawValue ?? "", "visible": slot.isVisible,
            "active": NSApp.isActive, "keyWindow": NSApp.keyWindow?.title ?? "",
            "windows": NSApp.windows.filter(\.isVisible).count,
            "palette": popup.isShown, "views": views,
            "hideOnFocusLoss": settings.hideOnFocusLoss,
            "escHides": { () -> [String: Bool] in
                let lines = readConfigText().map(configLines)
                return Dictionary(uniqueKeysWithValues: SharedWindow.escViews.map { ($0.rawValue, escHideCount($0, lines: lines) > 0) })
            }(),
            "pid": Int(getpid()),
            "paletteCommands": paletteCommands().map { $0.label ?? $0.name },
            "terminalPanel": terminalPanel?.testState() ?? ["shown": false],
            "viewSwitcher": ["shown": viewSwitcher?.window.isShown ?? false,
                             "rows": viewSwitcher?.window.rows.compactMap { ($0 as? SwitchViewRow).map { "\($0.name)|\($0.location)" } } ?? [],
                             "selection": viewSwitcher?.window.selection ?? 0],
            "paths": pathsWindow?.testState ?? ["shown": false,
                                                "rows": PathShelf.shared.entries().map { ["path": $0.path, "why": $0.why.rawValue] }],
            "headerStyle": HeaderStyle.current.rawValue,
            "pane": slot.current.flatMap { slotMember($0)?.slotWindow }.map { PaneNav.shared.testState($0) } ?? [:],
            "screenshot": screenshot.testState,
            "compare": { () -> [String: Any] in
                var st = CompareWindow.current?.testState ?? ["shown": false, "sessions": [Any]()]
                st["view"] = slot.current?.isCompare == true ? slot.current!.rawValue : ""
                if let sub = CompareWindow.sub { st["subView"] = sub.testState }
                return st
            }(),
            "paneShot": screenshot.paneShotLast,
            "activations": appActivations,
            "frontmostPid": Int(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0),
            "tools": Dictionary(subWindows.filter(\.config.toolPanel).map { w -> (String, Any) in
                var st = w.testState
                st["wid"] = w.nativeWindow.windowNumber
                st["level"] = w.nativeWindow.level.rawValue
                return (w.config.name, st)
            }, uniquingKeysWith: { a, _ in a }),
        ]
        guard let d = try? JSONSerialization.data(withJSONObject: state, options: [.sortedKeys]) else { return "{}" }
        return String(decoding: d, as: UTF8.self)
    }

    private func startCommandServer() {
        let socketPath = popupTmpDir() + settings.notesSocketName
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let fd = listenUnixSocket(socketPath) else { return }
            while true {
                let cfd = Darwin.accept(fd, nil, nil)
                guard cfd >= 0 else { continue }
                var noSigPipe: Int32 = 1
                setsockopt(cfd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))
                var tv = timeval(tv_sec: Int(serverRecvTimeout), tv_usec: 0)
                setsockopt(cfd, SOL_SOCKET, SO_RCVTIMEO, &tv,
                           socklen_t(MemoryLayout<timeval>.size))
                var buf = [UInt8](repeating: 0, count: 2048)
                let n = read(cfd, &buf, buf.count)
                let query = n > 0 ? (String(bytes: buf[..<n], encoding: .utf8) ?? "")
                    .trimmingCharacters(in: .whitespacesAndNewlines) : ""
                if query == "ping" {
                    close(cfd)
                    continue
                }
                if query == "screenshot-permission" {
                    let line = (ScreenshotController.permitted ? "granted" : "denied") + "\n"
                    line.withCString { _ = Darwin.write(cfd, $0, strlen($0)) }
                    close(cfd)
                    continue
                }
                if query == "screenshot" || query.hasPrefix("screenshot\t") {
                    let words = query.split(separator: "\t").dropFirst().map(String.init)
                    let wantsReply = (try? ShotArgs.parse(words).get())?.wantsReply ?? false
                    if !wantsReply { close(cfd) }
                    DispatchQueue.main.async { [weak self] in
                        guard let self else { if wantsReply { close(cfd) }; return }
                        self.screenshot.handle(words, reply: wantsReply ? { data in
                            DispatchQueue.global(qos: .userInitiated).async {
                                if let data, !data.isEmpty { writeAll(cfd, data) }
                                close(cfd)
                            }
                        } : nil)
                    }
                    continue
                }
                if query.hasPrefix("compare\t") {
                    let words = query.split(separator: "\t", omittingEmptySubsequences: false).dropFirst().map(String.init)
                    let wait = words.contains("--wait")
                    if !wait { close(cfd) }
                    DispatchQueue.main.async { [weak self] in
                        guard let self else { if wait { close(cfd) }; return }
                        var done = false
                        self.handleCompareMessage(words, reply: wait ? {
                            guard !done else { return }
                            done = true
                            DispatchQueue.global(qos: .userInitiated).async {
                                writeAll(cfd, Data("done\n".utf8))
                                close(cfd)
                            }
                        } : nil)
                    }
                    continue
                }
                if query == "pane-shot" || query.hasPrefix("pane-shot\t") {
                    let words = query.split(separator: "\t").dropFirst().map(String.init)
                    DispatchQueue.main.async { [weak self] in
                        guard let self else { close(cfd); return }
                        self.screenshot.paneShot(words) { line in
                            DispatchQueue.global(qos: .userInitiated).async {
                                writeAll(cfd, Data((line + "\n").utf8))
                                close(cfd)
                            }
                        }
                    }
                    continue
                }
                if query == "reload" || query == "restart" {
                    var reply = "{\"ok\":false,\"error\":\"timeout\"}"
                    let done = DispatchSemaphore(value: 0)
                    DispatchQueue.main.async { [weak self] in
                        guard let self else { done.signal(); return }
                        if query == "restart" {
                            reply = "{\"ok\":true,\"restarting\":true}"
                            done.signal()
                            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self.restartDaemon() }
                            return
                        }
                        self.reloadConfig()
                        let issues: [[String: Any]] = (try? String(contentsOfFile: settings.commandsConfPath, encoding: .utf8))
                            .map { validateConfig($0).map { ["line": $0.line, "message": $0.message, "fatal": $0.fatal] } } ?? []
                        let obj: [String: Any] = ["ok": !configUsingBackup, "commands": self.commands.count,
                                                  "usingBackup": configUsingBackup, "issues": issues]
                        if let d = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]) {
                            reply = String(decoding: d, as: UTF8.self)
                        }
                        done.signal()
                    }
                    _ = done.wait(timeout: .now() + 10)
                    let line = reply + "\n"
                    line.withCString { _ = Darwin.write(cfd, $0, strlen($0)) }
                    close(cfd)
                    continue
                }
                if query == "state" || query.hasPrefix("do:") {
                    var reply = "{\"error\":\"timeout\"}"
                    let done = DispatchSemaphore(value: 0)
                    DispatchQueue.main.async { [weak self] in
                        reply = self?.testQuery(query) ?? "{}"
                        done.signal()
                    }
                    _ = done.wait(timeout: .now() + 2)
                    let line = reply + "\n"
                    line.withCString { _ = Darwin.write(cfd, $0, strlen($0)) }
                    close(cfd)
                    continue
                }
                close(cfd)
                if n > 0 {
                    let name = query
                    let t0 = DispatchTime.now().uptimeNanoseconds
                    let prep = settings.sharedWindow && Self.hotkeyModes.contains(name)
                        ? Self.hotkeyPrep() : nil
                    DispatchQueue.main.async { [weak self] in
                        defer {
                            if Self.hotkeyModes.contains(name) {
                                let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
                                self?.log(String(format: "hotkey %@: %@, %.1f ms to shown", name, prep?.log ?? "", ms))
                            }
                            self?.slot.aerospaceCacheCleared = false
                        }
                        if let prep { self?.applyHotkeyPrep(prep) }
                        if name == "reset-size" {
                            self?.noteWindow?.resetToDefaultSize()
                        } else if name == "toggle-terminal" {
                            guard let w = self?.noteWindow else { return }
                            w.toggleTerminalDrawer()
                        } else if name.hasPrefix("open:") {
                            let path = String(name.dropFirst(5))
                            self?.openNoteFile((path as NSString).expandingTildeInPath)
                        } else if settings.sharedWindow && Self.hotkeyModes.contains(name) {
                            self?.toggleCommand(name)
                        } else if name == "notes" {
                            self?.showNotes()
                        } else if name == "term" {
                            self?.toggleTerminalPanel()
                        } else if name == "notes-find" || name == "notes-grep" {
                            self?.showNoteFind(grep: name == "notes-grep")
                        } else if name == "jira-dashboard" {
                            self?.showJiraDashboard()
                        } else if name == "setup" {
                            SetupWindow.show(controller: self)
                        } else if name == "confluence" || name == "confluence-setup" {
                            self?.showConfluence(setup: name == "confluence-setup")
                        } else if name == "ai" {
                            self?.showAI()
                        } else if name.hasPrefix("jira-poll-") || name == "jira-setup" {
                            guard let self else { return }
                            let on = jiraEnabledInConfig()
                            switch name {
                            case "jira-setup": self.showJiraSetup()
                            case "jira-poll-on": if !on { self.enableJiraChecked() }
                            case "jira-poll-off": if on { self.setJiraEnabled(false) }
                            default: self.toggleJiraPoll()
                            }
                        } else {
                            self?.showCommand(name)
                        }
                    }
                }
            }
        }
    }

    func show() {
        popup.show()
    }

    private func drawRow(_ rect: NSRect, _ row: PopupRow, _ selected: Bool) {
        if let r = row as? SplitRow { drawSplitRow(rect, r, selected); return }
        let z = popup.config.zoom
        let c = popup.config.colors
        let cy = rect.midY
        if selected {
            drawCursor(NSRect(x: 8 * z, y: cy - rowPillH * z / 2,
                              width: rect.width - 16 * z, height: rowPillH * z))
        }
        let ws = row as? WorkspaceRow
        let keyRect = NSRect(x: 14 * z, y: cy - 10 * z, width: 22 * z, height: 20 * z)
        let focused = ws?.focused ?? false
        let chip = NSBezierPath(roundedRect: keyRect, xRadius: 5 * z, yRadius: 5 * z)
        (focused ? c.accentOn : c.surface1).setFill()
        chip.fill()
        let keyAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 11 * z, weight: .bold),
            .foregroundColor: focused ? c.onAccent : (selected ? TEXT : TEXT.withAlphaComponent(0.88)),
        ]
        let key = row.title as NSString
        let ks = key.size(withAttributes: keyAttrs)
        key.draw(at: NSPoint(x: keyRect.midX - ks.width / 2, y: keyRect.midY - ks.height / 2),
                 withAttributes: keyAttrs)
        var right = rect.width - 14 * z
        if let n = ws?.unread, !n.isEmpty {
            right = drawBadge(n, rightEdge: right, cy: cy, z: z) - 6 * z
        }
        var ix: CGFloat = 44 * z
        for img in row.icons {
            popupDrawImage(img, in: NSRect(x: ix, y: cy - rowIconSize * z / 2,
                                           width: rowIconSize * z, height: rowIconSize * z))
            ix += rowIconStride * z
        }
        let text = ws?.match ?? row.trailing
        if let text, right - ix > 30 {
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byTruncatingTail
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 11 * z),
                .foregroundColor: ws?.match != nil ? TEXT.withAlphaComponent(0.92) : DIM,
                .paragraphStyle: para,
            ]
            let h = (text as NSString).size(withAttributes: attrs).height
            (text as NSString).draw(with: NSRect(x: ix + 4 * z, y: cy - h / 2,
                                                 width: right - ix - 8 * z, height: h),
                                    options: [.usesLineFragmentOrigin], attributes: attrs)
        }
    }

    private func drawCursor(_ pill: NSRect) {
        let z = popup.config.zoom
        let c = popup.config.colors
        let p = NSBezierPath(roundedRect: pill, xRadius: popup.config.buttonRadius * z,
                             yRadius: popup.config.buttonRadius * z)
        c.highlight.setFill()
        p.fill()
        c.accentOn.withAlphaComponent(0.55).setStroke()
        p.lineWidth = rowPillBorder * z
        p.stroke()
        NSGraphicsContext.current?.saveGraphicsState()
        p.addClip()
        c.accentOn.setFill()
        NSRect(x: pill.minX, y: pill.minY, width: 3 * z, height: pill.height).fill()
        NSGraphicsContext.current?.restoreGraphicsState()
    }

    @discardableResult
    private func drawBadge(_ text: String, rightEdge: CGFloat, cy: CGFloat, z: CGFloat) -> CGFloat {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 9.5 * z, weight: .semibold),
            .foregroundColor: NSColor.white,
        ]
        let ts = (text as NSString).size(withAttributes: attrs)
        let h = 15 * z
        let w = max(h, ts.width + 9 * z)
        let r = NSRect(x: rightEdge - w, y: cy - h / 2, width: w, height: h)
        popup.config.colors.tone(.danger).setFill()
        NSBezierPath(roundedRect: r, xRadius: h / 2, yRadius: h / 2).fill()
        (text as NSString).draw(at: NSPoint(x: r.midX - ts.width / 2, y: r.midY - ts.height / 2),
                                withAttributes: attrs)
        return r.minX
    }

    private func splitMid(_ width: CGFloat) -> CGFloat { (width * 0.46).rounded() }

    private func drawSplitRow(_ rect: NSRect, _ r: SplitRow, _ selected: Bool) {
        let z = popup.config.zoom
        let mid = splitMid(rect.width)
        TEXT.withAlphaComponent(0.1).setFill()
        NSRect(x: mid, y: rect.minY + 4 * z, width: 1, height: rect.height - 8 * z).fill()
        if let cmd = r.command {
            let on = selected && splitPane == 0
            let cell = NSRect(x: 8 * z, y: rect.midY - rowPillH * z / 2, width: mid - 16 * z, height: rowPillH * z)
            if on { drawCursor(cell) }
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byTruncatingTail
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 13 * z, weight: on ? .semibold : .regular),
                .foregroundColor: on ? TEXT : TEXT.withAlphaComponent(0.88),
                .paragraphStyle: para,
            ]
            let label = (cmd.label ?? cmd.name) as NSString
            let h = label.size(withAttributes: attrs).height
            label.draw(with: NSRect(x: cell.minX + 14 * z, y: cell.midY - h / 2,
                                    width: cell.width - 22 * z, height: h),
                       options: [.usesLineFragmentOrigin], attributes: attrs)
        }
        if let ws = r.workspace {
            NSGraphicsContext.current?.saveGraphicsState()
            let t = NSAffineTransform()
            t.translateX(by: mid + 1, yBy: 0)
            t.concat()
            drawRow(NSRect(x: 0, y: rect.minY, width: rect.width - mid - 1, height: rect.height),
                    ws, selected && splitPane == 1)
            NSGraphicsContext.current?.restoreGraphicsState()
        }
    }

    private static func slotCommand(_ name: String, label: String) -> CommandSpec {
        var c = CommandSpec(name: name)
        c.label = label
        c.aliases = configAliases(name)
        return c
    }

    func paletteCommands() -> [CommandSpec] {
        let jira = jiraEnabledInConfig()
        func listed(_ section: String) -> Bool {
            tri(configSectionValue(section, "in-palette")) ?? true
        }
        var all = commands.filter { $0.inPalette && ($0.name != "jira-config" || jira) }
        if confluenceEnabled(), listed("confluence") {
            all.append(Self.slotCommand("confluence", label: configSectionValue("confluence", "label") ?? "Confluence Search"))
        }
        if aiEnabled(), listed("ai") {
            all.append(Self.slotCommand("ai", label: configSectionValue("ai", "label") ?? "AI View"))
        }
        if compareEnabled(), listed("compare") {
            all.append(Self.slotCommand("compare", label: configSectionValue("compare", "label") ?? "Compare"))
        }
        if settings.sharedWindow {
            all.append(Self.slotCommand("window", label: "Kitchen Sink"))
        }
        let first = settings.paletteFirst.compactMap { n in all.first { $0.name.lowercased() == n } }
        return first + all.filter { c in !first.contains { $0.name == c.name } }
    }

    private func filter(_ query: String) -> [PopupRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if q != splitQuery {
            splitQuery = q
            splitPane = 0
            popup.selection = 0
        }
        let slash = q.hasPrefix("/")
        let t = slash ? String(q.dropFirst()).trimmingCharacters(in: .whitespaces) : q
        var wsRows: [WorkspaceRow] = []
        if !slash {
            let unread = status.unreadByApp
            var exact: [WorkspaceRow] = [], hits: [WorkspaceRow] = []
            for ws in workspaces {
                var row = WorkspaceRow(ws: ws, iconCache: &iconCache)
                row.unread = rowUnread(ws, unread)
                if t.isEmpty {
                    if !ws.apps.isEmpty || ws.focused { hits.append(row) }
                    continue
                }
                let id = ws.id.lowercased()
                let matched = ws.apps.filter {
                    $0.name.lowercased().contains(t) || ($0.windowTitle ?? "").lowercased().contains(t)
                }
                if id == t {
                    exact.append(row)
                } else if (!ws.apps.isEmpty && id.contains(t)) || !matched.isEmpty {
                    if !matched.isEmpty {
                        row.match = matched.prefix(2).map { a in
                            a.windowTitle.map { "\(a.name) — \($0)" } ?? a.name
                        }.joined(separator: " · ")
                    }
                    hits.append(row)
                }
            }
            wsRows = exact + hits
        }
        let cmds = t.isEmpty ? paletteCommands() : PopupFuzzy.filter(paletteCommands(), query: t) { c in
            (c.label.map { "\($0) \(c.name)" } ?? c.name) + " " + c.aliases.joined(separator: " ")
        }
        var out: [PopupRow] = []
        for i in 0..<max(cmds.count, wsRows.count) {
            out.append(SplitRow(command: i < cmds.count ? cmds[i] : nil,
                                workspace: i < wsRows.count ? wsRows[i] : nil))
        }
        if cmds.isEmpty && !wsRows.isEmpty { splitPane = 1 }
        return out
    }

    func refreshWorkspaceStrip() {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let fresh = gatherWorkspaces()
            DispatchQueue.main.async { [weak self] in
                guard let self, !fresh.isEmpty else { return }
                self.workspaces = fresh
                let unread = self.status.unreadByApp
                let cells = fresh.filter { !$0.apps.isEmpty || $0.focused }.map { ws -> PopupChrome.WorkspaceCell in
                    let row = WorkspaceRow(ws: ws, iconCache: &self.iconCache)
                    return PopupChrome.WorkspaceCell(key: ws.id, icons: Array(row.icons.prefix(3)),
                                                     extra: ws.apps.count > 3 ? "+\(ws.apps.count - 3)" : nil,
                                                     unread: self.rowUnread(ws, unread), focused: ws.focused)
                }
                let sig = cells.map { "\($0.key)|\($0.icons.count)|\($0.extra ?? "")|\($0.unread)|\($0.focused)" }.joined(separator: ";")
                guard sig != self.stripSig else { return }
                self.stripSig = sig
                PopupChrome.workspaceCells = cells
                PopupChrome.redrawAll()
            }
        }
    }

    func switchWorkspace(cell: Int) {
        guard PopupChrome.workspaceCells.indices.contains(cell) else { return }
        let key = PopupChrome.workspaceCells[cell].key
        DispatchQueue.global(qos: .userInitiated).async {
            _ = aerospaceCall(["workspace", key])
        }
    }

    private func rowUnread(_ ws: WorkspaceInfo, _ unread: [String: String]) -> String {
        var total = 0, dot = false
        for id in Set(ws.apps.compactMap(\.bundleID)) {
            guard let n = unread[id] else { continue }
            if let v = Int(n) { total += v } else { dot = true }
        }
        return total > 0 ? String(total) : dot ? "•" : ""
    }

    private func refreshRows() {
        guard popup.isShown else { return }
        let sel = popup.selection
        popup.setRows(filter(popup.currentQuery))
        popup.selection = min(sel, max(0, popup.rows.count - 1))
    }

    private func switcherKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        guard popup.isShown, mods.intersection([.command, .option]).isEmpty else { return false }
        let ctrl = mods.contains(.control)
        switch (code, ctrl) {
        case (125, false), (45, true):
            moveCursor(1)
            return true
        case (126, false), (35, true):
            moveCursor(-1)
            return true
        case (48, false):
            switchPane(splitPane == 0 ? 1 : 0)
            return true
        case (123, false), (124, false):
            guard popup.currentQuery.isEmpty else { return false }
            switchPane(code == 124 ? 1 : 0)
            return true
        default:
            return false
        }
    }

    private func hasCell(_ i: Int, _ pane: Int) -> Bool {
        guard popup.rows.indices.contains(i), let r = popup.rows[i] as? SplitRow else { return false }
        return pane == 0 ? r.command != nil : r.workspace != nil
    }

    private func switchPane(_ pane: Int) {
        let lines = popup.rows.indices.filter { hasCell($0, pane) }
        guard let last = lines.last else { return }
        splitPane = pane
        popup.selection = lines.contains(popup.selection) ? popup.selection
            : (lines.first { $0 >= popup.selection } ?? last)
    }

    private func moveCursor(_ d: Int) {
        let idx = popup.rows.indices.filter { hasCell($0, splitPane) }
        guard !idx.isEmpty else { return }
        if let i = idx.firstIndex(of: popup.selection) {
            popup.selection = idx[(i + d + idx.count) % idx.count]
        } else {
            popup.selection = idx[0]
        }
    }

    private func clickRow(_ index: Int) {
        let rows = popup.rows
        guard rows.indices.contains(index) else { return }
        let row = rows[index]
        if row is SplitRow {
            let x = popup.nativeWindow.mouseLocationOutsideOfEventStream.x
            let width = popup.nativeWindow.contentView?.bounds.width ?? settings.switcherWidth
            let pane = x < splitMid(width) ? 0 : 1
            guard hasCell(index, pane) else { return }
            splitPane = pane
        }
        accept(row)
    }

    private func accept(_ row: PopupRow) {
        if let r = row as? SplitRow {
            if splitPane == 0, let c = r.command { accept(CommandRow(c)) }
            else if let w = r.workspace { accept(w) }
            return
        }
        if let cr = row as? CommandRow {
            if isToolPanel(cr.command) {
                popup.hide(restore: false)
                openTool(cr.command)
                return
            }
            switch cr.command.kind {
            case .shell:
                let cmd = cr.command
                popup.hide(restore: true)
                if cmd.name == "jira-config" {
                    showJiraDashboard()
                    break
                }
                if cmd.name == "confluence" { showConfluence(); break }
                if cmd.name == "ai" { showAI(); break }
                if cmd.name == "compare" { openSlotHere { $0.showCompare() }; break }
                if cmd.name == "window" { openSlotHere { $0.toggleCommand("window") }; break }
                commandRunner?.run(cmd.script ?? "") { out in
                    self.log("cmd '\(cmd.name)' -> \(out)")
                }
            case .note:
                let wid = savedWID
                let pid = savedPID
                if settings.sharedWindow {
                    popup.hide(restore: false)
                    slot.open(.notes)
                    break
                }
                focusExistingOrOpen(named: cr.command.windowName) {
                    openNoteWindow(cr.command, restoreWID: wid, restorePID: pid)
                }
            case .list:
                let wid = savedWID
                let pid = savedPID
                if settings.sharedWindow && cr.command.name == "jira" {
                    popup.hide(restore: false)
                    slot.open(.jira)
                    break
                }
                focusExistingOrOpen(named: cr.command.windowName) {
                    openListWindow(cr.command, restoreWID: wid, restorePID: pid)
                }
            case .output:
                popup.hide(restore: true)
                openOutputWindow(cr.command)
            case .files:
                popup.hide(restore: false)
                if settings.sharedWindow {
                    slotShowFiles()
                    break
                }
                let wid = savedWID
                let pid = savedPID
                if let existing = subWindows.first(where: {
                    $0.config.name == cr.command.windowName
                }) {
                    focusSubWindow(existing)
                } else {
                    openFilesWindow(cr.command, restoreWID: wid, restorePID: pid)
                }
            }
            return
        }
        if let wr = row as? WorkspaceRow {
            popup.hide(restore: false)
            DispatchQueue.global(qos: .userInitiated).async {
                _ = aerospaceCall(["workspace", wr.title])
            }
        }
    }

    func log(_ s: String) { wsLog(s) }

    private func copy(_ text: String, _ what: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        log("copied \(what)")
    }

    func toggleNotes() {
        if let cmd = commands.first(where: { $0.kind == .note }),
           let w = subWindows.first(where: { $0.config.name == cmd.windowName }),
           w.isShown, w.nativeWindow.isKeyWindow {
            w.hide(restore: true)
            return
        }
        showNotes()
    }

    func toggleCommand(_ name: String) {
        let name = name == "voice" ? "notes" : name
        if name == "window" {
            guard settings.sharedWindow else { toggleNotes(); return }
            let (wid, pid) = readFocusFile()
            if !slot.isVisible { (savedWID, savedPID) = (wid, pid) }
            slot.toggle(userInIt: userInOurWindow(pid, name))
            return
        }
        if name == "confluence" {
            guard confluenceEnabled() else {
                log("hotkey confluence: [confluence] enabled is not true — ignored")
                return
            }
            guard settings.sharedWindow else { showConfluence(); return }
            let (wid, pid) = readFocusFile()
            if !slot.isVisible { (savedWID, savedPID) = (wid, pid) }
            slot.hotkey(.confluence, userInIt: userInOurWindow(pid, name))
            return
        }
        if name == "compare" {
            guard compareEnabled() else {
                log("compare: [compare] enabled is not true — ignored")
                return
            }
            guard settings.sharedWindow else { showCompare(); return }
            let (wid, pid) = readFocusFile()
            if !slot.isVisible { (savedWID, savedPID) = (wid, pid) }
            slot.hotkey(.compare, userInIt: userInOurWindow(pid, name))
            return
        }
        if name == "ai" {
            guard aiEnabled() else {
                log("hotkey ai: [ai] enabled is not true — ignored")
                return
            }
            guard settings.sharedWindow else { showAI(); return }
            let (wid, pid) = readFocusFile()
            if !slot.isVisible { (savedWID, savedPID) = (wid, pid) }
            slot.hotkey(.ai, userInIt: userInOurWindow(pid, name))
            return
        }
        let kind = commands.first(where: { $0.name == name })?.kind
        if settings.sharedWindow, name == "jira" || kind == .note || kind == .files {
            let (wid, pid) = readFocusFile()
            if !slot.isVisible { (savedWID, savedPID) = (wid, pid) }
            let inIt = userInOurWindow(pid, name)
            if kind == .files {
                slotShowFiles(hotkey: true, userInIt: inIt)
            } else {
                slot.hotkey(name == "jira" ? .jira : .notes, userInIt: inIt)
            }
            return
        }
        if let w = subWindows.first(where: { $0.config.name == name }),
           w.isShown, w.nativeWindow.isKeyWindow {
            w.hide(restore: true)
            return
        }
        showCommand(name)
    }

    private func userInOurWindow(_ focusPID: pid_t?, _ name: String) -> Bool? {
        let keyed = NSApp.isActive && NSApp.keyWindow?.isVisible == true
        guard let focusPID else { return nil }
        if (NSApp.keyWindow?.delegate as? PopupWindow)?.config.toolPanel == true { return false }
        let inIt = focusPID == getpid() && keyed
        if focusPID == getpid() && !keyed {
            log("hotkey \(name): aerospace says our window is focused but AppKit has no key window — focusing, not hiding")
        }
        return inIt
    }

    func toggleVimModeForNotes() {
        guard let idx = commands.firstIndex(where: { $0.name == "notes" }) else { return }
        let cmd = commands[idx]
        let newValue = !cmd.vimMode
        log("vim mode: \(newValue ? "enabled" : "disabled") for notes")

        commands[idx].vimMode = newValue

        saveConfigValue(section: "notes", key: "vim-mode", value: newValue ? "true" : "false")

        rebuildNoteWindow()
    }

    func vimArgs(for cmd: CommandSpec, socket: String, file: String?) -> [String] {
        var a: [String] = []
        let isNvim = (cmd.vimBin as NSString).lastPathComponent.hasPrefix("nvim")
        if isNvim { a += ["--listen", socket] }
        let custom = cmd.vimInit.map { ($0 as NSString).expandingTildeInPath }
        if let custom, FileManager.default.fileExists(atPath: custom) {
            a += ["-u", custom]
        } else {
            let bundled = assetDir + "/vim/init.lua"
            if FileManager.default.fileExists(atPath: bundled) {
                a += ["-u", bundled]
            }
        }
        func rgb(_ c: NSColor) -> String {
            let cc = c.usingColorSpace(.sRGB) ?? c
            return String(format: "#%02X%02X%02X",
                          Int(round(cc.redComponent * 255)),
                          Int(round(cc.greenComponent * 255)),
                          Int(round(cc.blueComponent * 255)))
        }
        let imgFile = (socket as NSString).deletingPathExtension + ".images.json"
        a += ["--cmd", "let g:ws_sock='\(popupTmpDir() + settings.notesSocketName)'",
              "--cmd", "let g:ws_img_file='\(imgFile)'",
              "--cmd", "let g:ws_img_rows=\(max(1, cmd.imageRows))"]
        let sel = (cmd.highlightColor ?? GROUP_BG).usingColorSpace(.sRGB) ?? GROUP_BG
        let card = (cmd.backgroundColor ?? BAR).withAlphaComponent(1).usingColorSpace(.sRGB) ?? BAR
        let lets = ["let g:ws_fg='\(rgb(cmd.textColor ?? TEXT))'",
                    "let g:ws_dim='\(rgb(cmd.dimColor ?? DIM))'",
                    "let g:ws_sel='\(rgb(cmd.highlightColor ?? GROUP_BG))'",
                    "let g:ws_line='\(rgb(sel.blended(withFraction: 0.5, of: card) ?? sel))'"]
            + PopupWindow.vimPaletteLets(windowColors(cmd))
        a += ["--cmd", lets.joined(separator: " | ")]
        if let file { a.append(file) }
        return a
    }

    private func unregisterSubWindow(_ w: PopupWindow, restore: Bool,
                                     restoreWID: String?, restorePID: pid_t?) {
        let view = slotView(of: w)
        subWindows.removeAll { $0 === w }
        w.releaseHooks()
        if let view {
            slot.memberGone(view)
            return
        }
        guard restore else { return }
        if let pid = restorePID {
            NSRunningApplication(processIdentifier: pid)?.activate(
                options: [.activateAllWindows])
        }
        if let wid = restoreWID {
            DispatchQueue.global(qos: .userInitiated).async {
                _ = aerospaceCall(["focus", "--window-id", wid])
            }
        }
    }

    private func openRow(_ row: FieldRow, cmd: CommandSpec, isJira: Bool) {
        if isJira && jiraIsReleaseRow(row) {
            showJiraReleaseView(row)
        } else {
            showDetail(row, cmd: cmd)
        }
    }

    func jiraInspectorContent(_ row: FieldRow) -> PopupInspectorContent {
        func f(_ k: String) -> String { row.fields[k] ?? "" }
        let status = f("status"), priority = f("priority")
        func statusTone(_ s: String) -> PopupTone {
            let l = s.lowercased()
            if ["done", "closed", "resolved", "released", "complete"].contains(where: l.contains) { return .success }
            if l.contains("review") || l.contains("test") || l.contains("block") { return .warning }
            if l.contains("progress") || l.contains("develop") { return .accent }
            return .dim
        }
        func priorityTone(_ s: String) -> PopupTone {
            let l = s.lowercased()
            if ["highest", "high", "critical", "blocker", "major"].contains(where: l.contains) { return .danger }
            if l.contains("medium") { return .warning }
            return .dim
        }
        var chips: [(text: String, tone: PopupTone)] = []
        if !status.isEmpty { chips.append((status, statusTone(status))) }
        if !priority.isEmpty { chips.append((priority, priorityTone(priority))) }
        let updated = f("updated").replacingOccurrences(of: "T", with: " ")
        return PopupInspectorContent(
            key: f("key").isEmpty ? row.title : f("key"),
            title: f("title").isEmpty ? (f("summary").isEmpty ? row.title : f("summary")) : f("title"),
            chips: chips,
            fields: [("Assignee", f("assignee")), ("Reporter", f("reporter")), ("Release", f("releaseLabel")),
                     ("Project", f("project")), ("Labels", f("labels")), ("Updated", String(updated.prefix(16)))],
            body: f("description"))
    }

    func showJiraReleaseView(_ release: FieldRow) {
        guard var rc = commands.first(where: { $0.name == "jira" }) else { return }
        let key = release.fields["key"] ?? ""
        JiraPoll.run("jira_poll.py", ["--release-view", key]) { [weak self] code, out, err in
            guard let self else { return }
            guard code == 0, let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any],
                  let dir = d["dir"] as? String else {
                self.log("jira: release view failed (exit \(code)) \(err)")
                return
            }
            let rels = d["releases"] as? [[String: Any]] ?? []
            if let old = self.subWindows.first(where: { $0.config.name == jiraReleasesWindow }) {
                if settings.sharedWindow, old.isShown { self.pendingSlotFrame = old.nativeWindow.frame }
                self.closeSlotWindow(old)
                self.subWindows.removeAll { $0 === old }
            }
            rc.name = jiraReleasesWindow
            rc.windowName = jiraReleasesWindow
            rc.chromeTitle = "Releases"
            rc.sources = [dir]
            self.pendingReleaseTab = rels.first { ($0["key"] as? String) == key }?["file"] as? String
            self.log("jira: release view -> \(key) (\(rels.count) release(s))")
            self.openListWindow(rc, restoreWID: nil, restorePID: nil)
            if settings.sharedWindow { self.slot.push(.releases) }
        }
    }

    private func showDetail(_ row: FieldRow, cmd: CommandSpec) {
        let key = row.fields["key"] ?? row.title
        let text = detailText(for: row, cmd: cmd)
        detailRow = row
        if let existing = subWindows.first(where: { $0.config.name == settings.detailWindowName }) {
            existing.setEditorText(text)
            existing.chromeHeaderTitle = nil
            showTicketPage(row, in: existing, cmd: cmd)
            if settings.sharedWindow {
                slot.push(.detail)
            } else {
                existing.nativeWindow.makeKeyAndOrderFront(nil)
                NSApp.activate(ignoringOtherApps: true)
            }
            return
        }
        var cfg = PopupConfig(name: settings.detailWindowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableDrag = true
        cfg.sticky = true
        cfg.floating = cmd.float ?? false
        cfg.escCloseCount = settings.sharedWindow ? 1 : max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
        cfg.width = defaultDetailSize.width
        cfg.height = defaultDetailSize.height
        cfg.fontName = cmd.font
        cfg.colors = windowColors(cmd)
        cfg.headerColor = cmd.headerColor ?? headerBlueSilver
        cfg.titlePill = false
        if let bg = cmd.backgroundColor { cfg.tintAlpha = (bg.usingColorSpace(.sRGB) ?? bg).alphaComponent }
        let w = PopupWindow(config: cfg)
        w.editorReadOnly = true
        w.editorText = text
        w.chromeHeaderTitle = nil
        w.headerIcon = jiraAppIcon
        w.copyConfigButtonLabel = ""
        w.copyPathButtonLabel = "copy key"
        w.headerButtons = [("open in browser", 10)]
        w.onChromeHeaderClick = { [weak self] in
            let k = self?.detailRow?.fields["key"] ?? key
            self?.copy(k, "jira key: \(k)")
        }
        w.onHeaderButton = { [weak self] id in
            guard id == 10, let self else { return }
            if let url = jiraBrowseURL(self.detailRow ?? row) {
                NSWorkspace.shared.open(url)
                self.log("detail: opened \(url.absoluteString) in browser")
            }
        }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: nil, restorePID: nil)
        }
        subWindows.append(w)
        if settings.sharedWindow {
            w.onEscape = { [weak self] in self?.slot.back(esc: true) }
            placeSlotWindow(w)
            w.show()
            slot.push(.detail)
        } else {
            w.show()
        }
        showTicketPage(row, in: w, cmd: cmd)
        log("detail window opened for \(key)")
    }

    private var ticketView: JiraTicketView?
    private func showTicketPage(_ row: FieldRow, in w: PopupWindow, cmd: CommandSpec) {
        guard cmd.name == "jira" || cmd.name == jiraReleasesWindow, row.fields["status"] != nil,
              !jiraIsReleaseRow(row) else {
            w.setPageOverlay(nil)
            w.copyPathButtonLabel = "copy key"
            if !w.headerButtons.contains(where: { $0.1 == 10 }) { w.headerButtons = [("open in browser", 10)] + w.headerButtons }
            return
        }
        let v = ticketView ?? JiraTicketView(frame: .zero)
        ticketView = v
        v.onAction = { [weak self] a in
            guard let self, let r = self.detailRow else { return }
            let k = r.fields["key"] ?? ""
            switch a {
            case "copy-key":
                self.copy(k, "jira key: \(k)")
                w.showToast("Copied \(k)", symbol: "doc.on.clipboard")
            case "copy-link":
                if let u = jiraBrowseURL(r) {
                    let t = r.fields["title"] ?? ""
                    self.copy(u.absoluteString + (t.isEmpty ? "" : " \(t)"), "jira link: \(k)")
                    w.showToast("Copied link to \(k)", symbol: "link")
                }
            case "open":
                if let u = jiraBrowseURL(r) { NSWorkspace.shared.open(u); self.log("detail: opened \(u.absoluteString)") }
            case _ where a.hasPrefix("url:"):
                if let u = URL(string: String(a.dropFirst(4))) { NSWorkspace.shared.open(u) }
            default: break
            }
        }
        w.copyPathButtonLabel = ""
        w.headerButtons = w.headerButtons.filter { $0.1 != 10 }
        w.textZoomKey = "textZoom.jira"
        w.setPageOverlay(v)
        let key = row.fields["key"] ?? ""
        let cms = JiraTicketPage.cachedComments(key) { [weak self, weak v] in
            guard let self, let v, self.ticketView === v, self.detailRow?.fields["key"] == key else { return }
            v.setComments(JiraTicketPage.cachedComments(key) {} ?? [])
        }
        v.show(JiraTicketPage.html(row, colors: w.config.colors, url: jiraBrowseURL(row)?.absoluteString,
                                   labels: JiraPoll.fieldLabels(), comments: cms),
               background: w.config.colors.base)
    }

    private func detailText(for row: FieldRow, cmd: CommandSpec) -> String {
        let shown = [cmd.primary, cmd.content, cmd.detail, cmd.trailing, cmd.body]
            .compactMap { $0 }
        var out: [String] = []
        let key = row.fields["key"] ?? ""
        let name = row.fields["summary"] ?? row.fields["title"] ?? row.title
        let head = [key, name].filter { !$0.isEmpty }.joined(separator: " — ")
        if !head.isEmpty { out.append(head); out.append("") }
        var used = Set<String>()
        if row.fields["status"] != nil {
            used.formUnion(["key", "title", "summary"])
            func line(_ keys: [(String, String)]) {
                let parts = keys.compactMap { k, label -> String? in
                    guard let v = row.fields[k], !v.isEmpty else { return nil }
                    used.insert(k)
                    return "\(label): \(v)"
                }
                if !parts.isEmpty { out.append(parts.joined(separator: "   ·   ")); out.append("") }
            }
            line([("status", "Status"), ("priority", "Priority"), ("releaseLabel", "Release")])
            line([("assignee", "Assignee"), ("reporter", "Reporter"), ("project", "Project"), ("updated", "Updated")])
            if let d = row.fields["description"], !d.isEmpty {
                used.insert("description")
                out.append("Description")
                out.append(String(repeating: "─", count: 11))
                out.append(d)
                out.append("")
            }
        } else {
            for k in shown {
                if let v = row.fields[k], !v.isEmpty {
                    out.append("\(k): \(v)")
                    out.append("")
                }
            }
            used.formUnion(shown)
        }
        out.append("--- all fields ---")
        for (k, v) in row.fields.sorted(by: { $0.key < $1.key })
        where !k.hasPrefix("__") && !used.contains(k) && !v.isEmpty {
            out.append("\(k): \(v)")
        }
        return out.joined(separator: "\n")
    }

    func isToolPanel(_ cmd: CommandSpec) -> Bool {
        ["filefast", "paths", "prettyprint", "screenshot", "terminal"].contains(cmd.name) || (cmd.kind == .output && cmd.panel)
    }

    func openTool(_ cmd: CommandSpec) {
        switch cmd.name {
        case "filefast": openFileFastWindow(cmd)
        case "paths": showPaths(cmd)
        case "prettyprint": openPrettyPrintWindow(cmd)
        case "screenshot": showScreenshot()
        case "terminal": toggleTerminalPanel()
        default: openOutputWindow(cmd)
        }
    }

    func showViewSwitcher() {
        if viewSwitcher == nil {
            let v = ViewSwitcherPanel(colors: windowColors())
            v.onPick = { [weak self] id in self?.slot.navClicked(id) }
            viewSwitcher = v
        }
        func tabName(_ w: PopupWindow?) -> String {
            guard let w, w.tabTitles.indices.contains(w.selectedTab) else { return "" }
            let t = w.tabTitles[w.selectedTab]
            return t.hasSuffix(".json") ? String(t.dropLast(5)) : t
        }
        let filesWin = slotMember(.files) as? PopupWindow
        let names: [Int: String] = [SharedWindow.navFiles: "Files", SharedWindow.navNotes: "Notes",
                                    SharedWindow.navJira: "Jira", SharedWindow.navConfluence: "Confluence",
                                    SharedWindow.navCompare: "Compare", SharedWindow.navAI: "AI"]
        let views = SharedWindow.navIcons.map { icon -> (id: Int, name: String, icon: NSImage, location: String, aliases: [String]) in
            let loc: String
            switch icon.id {
            case SharedWindow.navFiles: loc = filesWin?.fileBrowser?.whereText ?? ""
            case SharedWindow.navNotes: loc = tabName(noteWindow)
            case SharedWindow.navJira: loc = tabName(slotMember(.jira) as? PopupWindow)
            case SharedWindow.navConfluence: loc = ConfluenceWindow.current?.whereText ?? ""
            case SharedWindow.navCompare: loc = CompareWindow.current?.whereText ?? ""
            case SharedWindow.navAI: loc = AIWindow.current?.whereText ?? ""
            default: loc = ""
            }
            let section: [Int: String] = [SharedWindow.navFiles: "files", SharedWindow.navNotes: "notes",
                                          SharedWindow.navJira: "jira", SharedWindow.navConfluence: "confluence",
                                          SharedWindow.navCompare: "compare", SharedWindow.navAI: "ai"]
            return (icon.id, names[icon.id] ?? icon.tip, icon.image, loc, section[icon.id].map(configAliases) ?? [])
        }
        let cur = slot.current.flatMap { SharedWindow.navOn($0) }
        viewSwitcher?.show(views, current: cur, preselect: slot.previousNav ?? cur,
                           over: slot.current.flatMap { slotMember($0)?.slotWindow })
    }

    func toggleTerminalPanel() {
        if terminalPanel == nil {
            let t = TerminalPanel(colors: windowColors(commands.first { $0.name == "terminal" }),
                                  shell: settings.shell, args: settings.shellArgs,
                                  fontName: settings.terminalFont, fontSize: settings.terminalFontSize)
            t.log = { [weak self] in self?.log($0) }
            terminalPanel = t
        }
        terminalPanel?.toggle()
    }

    func showScreenshot() {
        screenshot.trigger(ShotArgs(), extraDelay: 0.15)
    }

    private func raiseToolPanel(_ w: PopupWindow) {
        let win = w.nativeWindow
        win.orderFrontRegardless()
        win.makeKey()
        log("tool '\(w.config.name)' refocused")
    }

    private func reclaimToolKey(_ w: PopupWindow, then focus: (() -> Void)? = nil) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak w] in
            guard let w, w.isShown, !w.nativeWindow.isKeyWindow,
                  let k = NSApp.keyWindow, k !== w.nativeWindow else { return }
            w.nativeWindow.orderFrontRegardless()
            w.nativeWindow.makeKey()
            focus?()
        }
    }

    private func openOutputWindow(_ cmd: CommandSpec) {
        let shared = settings.sharedWindow && !cmd.panel
        if shared {
            if let old = currentOutputName, old != cmd.windowName,
               let ow = subWindows.first(where: { $0.config.name == old }) {
                closeSlotWindow(ow)
            }
            currentOutputName = cmd.windowName
        }
        if let existing = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            if shared {
                slot.push(.output)
                runOutput(cmd, into: existing)
                return
            }
            if cmd.panel {
                raiseToolPanel(existing)
            } else {
                existing.nativeWindow.makeKeyAndOrderFront(nil)
                NSApp.activate(ignoringOtherApps: true)
            }
            runOutput(cmd, into: existing)
            return
        }
        var cfg = PopupConfig(name: cmd.windowName)
        cfg.opaqueTabs = cmd.tabsOpaque ?? true
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.toolPanel = cmd.panel
        cfg.floating = cmd.float ?? cmd.panel
        cfg.escCloseCount = shared ? 1 : max(0, cmd.escClose ?? (cmd.panel ? 1 : settings.escClose))
        cfg.copyToast = settings.copyToast
        cfg.width = cmd.width > 0 ? cmd.width : defaultOutputSize.width
        cfg.height = cmd.height > 0 ? cmd.height : defaultOutputSize.height
        cfg.headerHeight = 30
        cfg.titlePill = false
        applyWindowTheme(&cfg, cmd)
        let w = PopupWindow(config: cfg)
        w.editorReadOnly = true
        w.editorText = "running \(cmd.name)…"
        w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
        w.headerIcon = cmd.icon ?? jiraAppIcon
        w.copyConfigButtonLabel = "copy config path"
        w.copyPathButtonLabel = "copy output"
        w.onChromeHeaderClick = { [weak self] in
            self?.copy(w.currentEditorText, "\(cmd.name) output")
        }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: nil, restorePID: nil)
        }
        subWindows.append(w)
        if shared {
            w.onEscape = { [weak self] in self?.slot.back(esc: true) }
            placeSlotWindow(w)
            w.show()
            slot.push(.output)
        } else {
            w.show()
            if cmd.panel { reclaimToolKey(w) }
        }
        runOutput(cmd, into: w)
    }

    private func runOutput(_ cmd: CommandSpec, into w: PopupWindow) {
        runScript(cmd.script ?? "", label: cmd.name, into: w)
    }

    private func openPrettyPrintWindow(_ cmd: CommandSpec) {
        let windowName = "prettyprint"
        if let existing = subWindows.first(where: { $0.config.name == windowName }) {
            raiseToolPanel(existing)
            return
        }
        var cfg = PopupConfig(name: windowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableDrag = true
        cfg.sticky = true
        cfg.toolPanel = true
        cfg.floating = cmd.float ?? true
        cfg.width = 1000
        cfg.height = 600
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = headerBlueSilver
        cfg.colors = windowColors()
        let w = PopupWindow(config: cfg)
        w.editorReadOnly = false
        w.editorText = ""
        w.chromeHeaderTitle = nil
        w.headerIcon = notesAppIcon
        w.itemCount = "paste JSON or XML below — auto-formats"
        w.copyConfigButtonLabel = ""
        w.copyPathButtonLabel = ""
        w.headerButtons = [("save file", 11), ("copy contents", 10)]
        w.onEditorTextChange = { [weak self, weak w] in
            guard let self, let w else { return }
            self.prettyFormatWorkItem?.cancel()
            let work = DispatchWorkItem { [weak self, weak w] in
                guard let self, let w else { return }
                let raw = w.currentEditorText
                DispatchQueue.global(qos: .userInitiated).async { [weak self, weak w] in
                    let result = self?.prettyFormat(raw)
                    DispatchQueue.main.async { [weak w] in
                        guard let result, let w else { return }
                        if let formatted = result.formatted {
                            w.setEditorSyntaxHighlighted(formatted)
                            w.setStatus(nil, isError: false)
                        } else if let err = result.error {
                            w.setStatus(err, isError: true)
                        } else {
                            w.setStatus(nil, isError: false)
                        }
                    }
                }
            }
            self.prettyFormatWorkItem = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35, execute: work)
        }
        w.onHeaderButton = { [weak self, weak w] id in
            guard let self, let w else { return }
            if id == 10 {
                let contents = w.currentEditorText
                guard !contents.isEmpty else { return }
                self.copy(contents, "prettyprint contents")
            } else if id == 11 {
                self.savePrettyPrint(w, dir: cmd.saveDir)
            }
        }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.prettyFormatWorkItem?.cancel()
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: nil, restorePID: nil)
        }
        subWindows.append(w)
        w.show()
        reclaimToolKey(w)
        log("prettyprint window opened")
    }

    private func openFileFastWindow(_ cmd: CommandSpec) {
        let windowName = "filefast"
        if let existing = subWindows.first(where: { $0.config.name == windowName }) {
            raiseToolPanel(existing)
            return
        }
        let colors = windowColors(cmd)
        var cfg = PopupConfig(name: windowName)
        cfg.enableToggle = false
        cfg.enableDrag = false
        cfg.dynamicHeight = true
        cfg.sticky = cmd.sticky
        cfg.toolPanel = true
        cfg.floating = cmd.float ?? true
        cfg.width = 620
        cfg.colors = colors
        cfg.showSearchBar = true
        cfg.searchWidthFraction = 0.42
        cfg.searchPlaceholder = "file name"
        let w = PopupWindow(config: cfg)
        w.onFilter = { _ in [] }

        let day = DateFormatter()
        day.dateFormat = "yyyy_MM_dd"
        let root = ((cmd.saveDir == "/tmp/" ? "/tmp/filefast" : cmd.saveDir) as NSString).expandingTildeInPath
        let dir = (root as NSString).appendingPathComponent(day.string(from: Date()))

        let paste = FileFastPasteView()
        paste.isRichText = false
        paste.drawsBackground = false
        paste.font = NSFont.systemFont(ofSize: cfg.inputFontSize)
        paste.textColor = .clear
        paste.insertionPointColor = colors.dim
        paste.textContainerInset = NSSize(width: 6, height: 4)
        paste.isVerticallyResizable = true
        paste.autoresizingMask = [.width]
        let scroll = NSScrollView()
        scroll.documentView = paste
        scroll.hasVerticalScroller = false
        scroll.drawsBackground = false
        scroll.wantsLayer = true
        scroll.layer?.backgroundColor = ButtonStyle.inputFill(colors).cgColor
        scroll.layer?.cornerRadius = 6
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = ButtonStyle.inputStroke(colors).cgColor
        scroll.autoresizingMask = [.width]
        let hint = NSTextField(labelWithString: "paste output")
        hint.font = NSFont.systemFont(ofSize: cfg.inputFontSize)
        hint.textColor = colors.dim
        paste.hint = hint
        paste.placeholderColor = colors.dim
        paste.pastedColor = colors.tone(.success)
        hint.frame = NSRect(x: 8, y: 3, width: 460, height: 16)
        scroll.addSubview(hint)

        let save: () -> Void = { [weak self, weak w, weak paste] in
            guard let self, let w, let paste else { return }
            let contents = paste.string
            let name = w.currentSearchText.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !name.isEmpty, !name.contains("/") else { w.focusSearchField(); NSSound.beep(); return }
            guard !contents.isEmpty else { w.nativeWindow.makeFirstResponder(paste); NSSound.beep(); return }
            let script = cmd.script ?? ""
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                var env = ProcessInfo.processInfo.environment
                env["FF_DIR"] = dir
                env["FF_NAME"] = name
                let out: String, ok: Bool
                do {
                    let r = try runProcess("/bin/bash", ["-c", script], stdin: contents, env: env, mergeStderr: true)
                    (out, ok) = (r.out, r.code == 0)
                } catch {
                    (out, ok) = (error.localizedDescription, false)
                }
                DispatchQueue.main.async { [weak self, weak w] in
                    self?.log("filefast \(ok ? "saved" : "failed"): \(out.trimmingCharacters(in: .whitespacesAndNewlines))")
                    guard ok, let w else { NSSound.beep(); return }
                    let path = out.trimmingCharacters(in: .whitespacesAndNewlines)
                        .split(separator: "\n").last.map(String.init) ?? name
                    if path.hasPrefix("/") { PathShelf.shared.add([path], why: .filefast) }
                    w.showToast("Copied \(path) to clipboard", symbol: "checkmark.circle.fill", centered: true)
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.3) { [weak w] in if let w, w.isShown { w.hide(restore: true) } }
                }
            }
        }
        w.onKeyPreview = { [weak w, weak paste] code, mods in
            guard let w, let paste else { return false }
            let inPaste = w.nativeWindow.firstResponder === paste
            if mods.contains(.command) {
                let nameTyped = !w.currentSearchText.isEmpty
                switch code {
                case 9 where inPaste || nameTyped:
                    w.nativeWindow.makeFirstResponder(paste)
                    paste.paste(nil); return true
                case 0 where inPaste: paste.selectAll(nil); return true
                case 8 where inPaste: paste.copy(nil); return true
                case 7 where inPaste: paste.cut(nil); return true
                case 6 where inPaste: paste.undoManager?.undo(); return true
                case 1:
                    save(); return true
                default: break
                }
            }
            switch code {
            case 53:
                w.hide(restore: true); return true
            case 36 where !(mods.contains(.shift) && inPaste):
                save(); return true
            case 48 where !mods.contains(.control):
                if inPaste { w.focusSearchField() } else { w.nativeWindow.makeFirstResponder(paste) }
                return true
            default: return false
            }
        }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.unregisterSubWindow(w, restore: restore, restoreWID: nil, restorePID: nil)
        }
        subWindows.append(w)
        w.show()
        if let backdrop = w.nativeWindow.contentView {
            let f = w.searchFieldFrame
            let x = f.maxX + 8
            scroll.frame = NSRect(x: x, y: f.minY, width: backdrop.bounds.width - x - f.minX, height: f.height)
            paste.frame = NSRect(origin: .zero, size: scroll.contentSize)
            paste.minSize = NSSize(width: 0, height: scroll.contentSize.height)
            paste.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
            paste.textContainer?.widthTracksTextView = true
            backdrop.addSubview(scroll)
        }
        w.focusSearchField()
        reclaimToolKey(w) { [weak w] in w?.focusSearchField() }
        log("filefast window opened")
    }

    private let prettySaveStamp: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "yyyyMMdd-HHmmss"
        return f
    }()

    private func savePrettyPrint(_ w: PopupWindow, dir: String) {
        let contents = w.currentEditorText
        guard !contents.isEmpty else { return }
        let t = contents.trimmingCharacters(in: .whitespacesAndNewlines)
        let ext: String
        if let f = t.first {
            if f == "<" { ext = "xml" }
            else if f == "{" || f == "[" { ext = "json" }
            else { ext = "txt" }
        } else {
            ext = "txt"
        }
        let absDir = (dir as NSString).expandingTildeInPath
        let path = (absDir as NSString)
            .appendingPathComponent("prettyprint-\(prettySaveStamp.string(from: Date())).\(ext)")
        do {
            try contents.write(toFile: path, atomically: true, encoding: .utf8)
        } catch {
            w.setStatus("save failed: \(error.localizedDescription)", isError: true)
            return
        }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(path, forType: .string)
        w.setStatus("saved: \(path)", isError: false)
        w.showToast("Copied \((path as NSString).abbreviatingWithTildeInPath) to clipboard", symbol: "doc.on.clipboard")
        log("prettyprint saved to \(path)")
    }

    private struct FormatResult {
        let formatted: String?
        let error: String?
    }

    private func prettyFormat(_ raw: String) -> FormatResult {
        let t = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = t.first else { return FormatResult(formatted: nil, error: nil) }
        if first == "{" || first == "[" {
            let (out, err) = runFormatter(tool: "jq", paths: ["/opt/homebrew/bin/jq",
                                                               "/usr/local/bin/jq"],
                                          args: ["."], input: raw)
            if let e = trimmed(err) {
                return FormatResult(formatted: nil, error: e)
            }
            return FormatResult(formatted: out.isEmpty ? raw : out, error: nil)
        }
        if first == "<" {
            let (out, err) = runFormatter(tool: "xmllint",
                                          paths: ["/usr/bin/xmllint", "/opt/homebrew/bin/xmllint"],
                                          args: ["--format", "-"], input: raw)
            if let e = trimmed(err) {
                return FormatResult(formatted: nil, error: e)
            }
            return FormatResult(formatted: out.isEmpty ? raw : out, error: nil)
        }
        return FormatResult(formatted: nil, error: nil)
    }

    private func runFormatter(tool: String, paths: [String], args: [String],
                              input: String) -> (String, String) {
        let exe = paths.first(where: { FileManager.default.isExecutableFile(atPath: $0) })
        do {
            let r = try runProcess(exe ?? "/usr/bin/env", exe == nil ? [tool] + args : args, stdin: input)
            return (r.out, r.err)
        } catch {
            return ("", "\(tool): \(error.localizedDescription)")
        }
    }

    private func trimmed(_ s: String) -> String? {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }

    private func runScript(_ script: String, label: String, into w: PopupWindow) {
        let shell = settings.shell
        DispatchQueue.global(qos: .userInitiated).async { [weak self, weak w] in
            let r: ProcessOutput
            do {
                r = try runProcess(shell, ["-c", (script as NSString).expandingTildeInPath])
            } catch {
                DispatchQueue.main.async { w?.setEditorText("failed to run: \(error)") }
                return
            }
            let shown = r.out + (r.err.isEmpty ? "" : "\n-- stderr --\n" + r.err) + "\n(exit \(r.code))"
            DispatchQueue.main.async { [weak self] in
                guard let w else { return }
                w.setEditorANSI(shown)
                self?.log("output '\(label)': exit \(r.code)")
            }
        }
    }

    private func openNoteWindow(_ cmd: CommandSpec,
                                restoreWID: String?, restorePID: pid_t?) {
        log("openNoteWindow: '\(cmd.name)' terminal=\(cmd.terminal) paths=\(cmd.paths.count)")
        guard !cmd.paths.isEmpty else {
            log("note '\(cmd.name)': no path configured")
            return
        }
        let paths = notePaths(cmd)
        let vimSocket = NSHomeDirectory()
            + "/.cache/kitchen-sink/nvim-\(cmd.name)-\(getpid()).sock"
        let cfg = noteWindowConfig(cmd, firstNote: paths[0], vimSocket: vimSocket)
        let w = PopupWindow(config: cfg)
        let session = NoteSession(host: self, cmd: cmd, window: w, paths: paths, vimSocket: vimSocket)
        session.install()
        let fb = makeFileBrowser(cfg, startDir: noteDir(session.currentPath), in: w,
                                 tag: "note '\(cmd.name)'", opened: "browser opened")
        w.installFileBrowser(fb, drawer: true)
        w.setHeaderButtonOn(10, w.terminalShown)
        w.setHeaderButtonOn(20, w.fileBrowserShown)
        subWindows.append(w)
        if settings.sharedWindow {
            w.onEscape = { [weak self] in self?.slot.hide("Esc / Cmd+W (notes)") }
            placeSlotWindow(w)
        }
        w.quietShow = slotPrewarming
        w.show()
    }

    private func notePaths(_ cmd: CommandSpec) -> [String] {
        var paths: [String] = []
        for p in expandPaths(cmd.paths, extensions: ["md"]) {
            if !FileManager.default.fileExists(atPath: p) {
                log("note '\(cmd.name)': \(p) deleted — dropping it and removing from config")
                removeNotePathFromConfig(p, section: cmd.name)
            } else if DismissedNotes.contains(p) {
                log("note '\(cmd.name)': \(p) dismissed — skipping")
            } else {
                paths.append(p)
            }
        }
        if paths.isEmpty {
            let first = (cmd.paths[0] as NSString).expandingTildeInPath
            let fallback = ensureDefaultNote(in: noteDir(first))
            paths = [fallback]
            addNotePathToConfig(fallback, section: cmd.name)
            log("note '\(cmd.name)': all listed notes deleted — opened fresh \(fallback)")
        }
        return paths
    }

    private func noteWindowConfig(_ cmd: CommandSpec, firstNote: String,
                                  vimSocket: String) -> PopupConfig {
        var cfg = PopupConfig(name: cmd.windowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableResize = cmd.resize
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.floating = false
        cfg.escCloseCount = max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
        cfg.tabs = true
        cfg.tabsAddButton = true
        cfg.tabsSidebarWidth = cmd.sidebarWidth
        cfg.opaqueTabs = cmd.tabsOpaque ?? true
        cfg.width = cmd.width > 0 ? cmd.width : defaultNoteSize.width
        cfg.fileBrowserDefault = cmd.startDrawer == "browser"
        cfg.terminalStartsOpen = cmd.startDrawer == "terminal"
        cfg.height = (cmd.height > 0 ? cmd.height : defaultNoteSize.height)
            + (cfg.fileBrowserDefault ? cfg.fileBrowserHeight
                                      : (cmd.terminal && cfg.terminalStartsOpen
                                         ? cmd.terminalHeight : 0))
        if cmd.maxHeight > 0 { cfg.maxHeight = cmd.maxHeight }
        cfg.terminal = cmd.terminal
        cfg.terminalHeight = cmd.terminalHeight
        if let td = cmd.terminalDir { cfg.terminalDir = td }
        cfg.fileBrowserBackground = cmd.browserBackground
            ?? THEME_BROWSER ?? cfg.fileBrowserBackground
        cfg.shell = settings.shell
        cfg.shellArgs = settings.shellArgs
        cfg.terminalFont = settings.terminalFont
        cfg.terminalFontSize = settings.terminalFontSize
        if cmd.fontSize > 0 { cfg.editorFontSize = cmd.fontSize }
        cfg.terminalBackground = cmd.terminalBackground
            ?? THEME_TERMINAL ?? cfg.terminalBackground
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.stretchHeaderButtons = !settings.sharedWindow
        applyWindowTheme(&cfg, cmd)
        cfg.terminalForeground = cmd.terminalForeground
        cfg.markdownImages = true
        if cmd.vimMode {
            let exe = resolveBinary(cmd.vimBin) ?? cmd.vimBin
            cfg.vimEditorExecutable = exe
            cfg.vimEditorSocket = vimSocket
            cfg.vimImageFile = (vimSocket as NSString).deletingPathExtension + ".images.json"
            cfg.vimEditorArgs = vimArgs(for: cmd, socket: vimSocket,
                                        file: noteIsPreview(firstNote) ? nil : firstNote)
            log("note '\(cmd.name)': vim pane \(exe) socket \(vimSocket)")
        }
        return cfg
    }

    private func saveNote(_ text: String, to path: String, cmd: CommandSpec) {
        do {
            try text.write(toFile: path, atomically: true, encoding: .utf8)
        } catch {
            log("note '\(cmd.name)': save failed: \(error)")
        }
        if (path as NSString).standardizingPath
            == (settings.commandsConfPath as NSString).standardizingPath {
            reportConfigEdit(text)
        }
    }

    private func reportConfigEdit(_ text: String) {
        let issues = validateConfig(text)
        if let f = issues.first(where: \.fatal) {
            let at = f.line > 0 ? "line \(f.line): " : ""
            noteWindow?.setStatus("commands.toml \(at)\(f.message) — the last good backup will be used until this is fixed",
                                  isError: true)
        } else if let w = issues.first {
            let more = issues.count > 1 ? " (+\(issues.count - 1) more)" : ""
            noteWindow?.setStatus("commands.toml line \(w.line): \(w.message)\(more)", isError: false)
        } else {
            noteWindow?.setStatus(nil, isError: false)
        }
    }

    private func notePathConfig(_ path: String) -> (lines: [String], display: String)? {
        guard let content = readConfigText() else {
            log("commands.toml: cannot read \(settings.commandsConfPath)")
            return nil
        }
        let home = NSHomeDirectory()
        let display = path.hasPrefix(home + "/")
            ? "~" + path.dropFirst(home.count)
            : path
        return (configLines(content), display)
    }

    private func notePathEntries(_ lines: [String], _ section: String)
        -> [(index: Int, key: String, value: String)] {
        configSectionEntries(lines, section).filter { $0.key == "paths" || $0.key == "path" }
    }

    private func addNotePathToConfig(_ path: String, section: String) {
        guard let (read, display) = notePathConfig(path) else { return }
        var lines = read
        guard let e = notePathEntries(lines, section).first else {
            log("commands.toml: no [\(section)] section to update")
            return
        }
        let parent = (path as NSString).deletingLastPathComponent
        let isListed = e.value.split(separator: ",").contains { entry in
            let s = (entry.trimmingCharacters(in: .whitespaces) as NSString).expandingTildeInPath
            return s == path || (s == parent && path.hasSuffix(".md"))
        }
        if isListed {
            log("commands.toml: \(display) already listed — no change")
            return
        }
        guard let line = configLine("paths", e.value.isEmpty ? display : e.value + ", " + display) else { return }
        lines[e.index] = line
        writeConfigText(lines.joined(separator: "\n"))
        log("commands.toml: added note \(display)")
    }

    private func removeNotePathFromConfig(_ path: String, section: String) {
        guard let (read, display) = notePathConfig(path) else { return }
        var lines = read
        for e in notePathEntries(lines, section) {
            let all = e.value.split(separator: ",")
            let kept = all.compactMap { entry -> String? in
                let s = String(entry).trimmingCharacters(in: .whitespaces)
                return (s as NSString).expandingTildeInPath == path ? nil : s
            }
            guard kept.count != all.count else { continue }
            if kept.isEmpty {
                lines.remove(at: e.index)
            } else {
                guard let line = configLine("paths", kept.joined(separator: ", ")) else { return }
                lines[e.index] = line
            }
            writeConfigText(lines.joined(separator: "\n"))
            log("commands.toml: removed \(display) from [\(section)]")
            return
        }
        log("commands.toml: no [\(section)] section to update")
    }

    enum ThemeScope {
        case window, notepad, terminal, browser
    }
    private var themePreviewDelegate: ThemePreviewDelegate?
    private var headerStylePreviewDelegate: HeaderStylePreviewDelegate?

    private func themeScopes(for w: PopupWindow) -> [(ThemeScope, String)] {
        var out: [(ThemeScope, String)] = [(.window, "Whole Window")]
        if w.config.editMode { out.append((.notepad, "Notepad Only")) }
        if w.config.editMode && w.hasTerminalDrawer { out.append((.terminal, "Terminal Only")) }
        if w.config.editMode && w.hasFileBrowser { out.append((.browser, "File Explorer Only")) }
        return out
    }

    func addThemeMenus(to menu: NSMenu, window w: PopupWindow, section: String) {
        let scopes = themeScopes(for: w)
        let themeMenu = NSMenu(title: "Theme")
        let currentBg = hexString(w.themeColor(.notepad).withAlphaComponent(1))
        let currentBrowser = hexString(w.themeColor(.browser).withAlphaComponent(1))
        let windowTextIsLight = w.config.colors.text.relativeLuminance > 0.45
        let presets = ThemePreset.all()
        let snapshot = ThemeSnapshot(w)
        let preview = ThemePreviewDelegate(
            onHighlight: { [weak self, weak w] tag in
                guard let self, let w else { return }
                if let tag, presets.indices.contains(tag) {
                    snapshot.restore(w)
                    self.applyThemePreset(presets[tag], scope: .window, to: w,
                                          section: section, persist: false)
                } else {
                    snapshot.restore(w)
                }
            },
            onClose: { [weak w] in if let w { snapshot.restore(w) } })
        themeMenu.delegate = preview
        themePreviewDelegate = preview
        let order = presets.indices.sorted {
            let a = presets[$0], b = presets[$1]
            if a.tone != b.tone { return a.tone.rawValue < b.tone.rawValue }
            return a.background.relativeLuminance > b.background.relativeLuminance
        }
        let toneTitles: [ThemePreset.Tone: String] = [.mid: "Mid Tones", .dark: "Dark", .light: "Light"]
        var lastTone: ThemePreset.Tone?
        for i in order {
            let p = presets[i]
            if p.tone != lastTone {
                if lastTone != nil { themeMenu.addItem(.separator()) }
                themeMenu.addItem(.sectionHeader(title: toneTitles[p.tone] ?? ""))
                lastTone = p.tone
            }
            let item: NSMenuItem
            if scopes.count == 1 {
                item = menuItem(p.name) { [weak self, weak w] in
                    guard let self, let w else { return }
                    preview.committed = true
                    snapshot.restore(w)
                    self.applyThemePreset(p, scope: .window, to: w, section: section)
                }
            } else {
                item = NSMenuItem(title: p.name, action: nil, keyEquivalent: "")
            }
            item.tag = i
            item.representedObject = i
            item.image = p.swatch()
            let matches = w.config.editMode ? currentBg == hexString(p.background)
                                            : currentBrowser == hexString(p.background)
            item.state = matches ? .on : .off
            let sub = NSMenu(title: p.name)
            sub.autoenablesItems = false
            for (scope, label) in scopes {
                let readable = scope != .browser || p.isLight != windowTextIsLight
                let si = menuItem(label, enabled: readable) { [weak self, weak w] in
                    guard let self, let w else { return }
                    preview.committed = true
                    snapshot.restore(w)
                    self.applyThemePreset(p, scope: scope, to: w, section: section)
                }
                if !readable {
                    si.toolTip = "Needs the window's text color — use Whole Window"
                }
                sub.addItem(si)
                if scope == .window && scopes.count > 1 { sub.addItem(.separator()) }
            }
            if scopes.count > 1 { item.submenu = sub }
            themeMenu.addItem(item)
        }
        themeMenu.addItem(.separator())
        themeMenu.addItem(menuItem("Custom Color…") { [weak self, weak w] in
            guard let self, let w else { return }
            preview.committed = true
            snapshot.restore(w)
            let roles: [PopupWindow.ThemeRole] = w.config.editMode
                ? [.terminal, .browser, .notepad, .header] : [.browser, .header]
            self.presentThemeRoleMenu(for: w, roles: roles, section: section)
        })
        themeMenu.addItem(menuItem("Reset to Defaults") { [weak self, weak w] in
            guard let self, let w else { return }
            preview.committed = true
            self.resetWindowTheme(w, section: section)
        })
        let themeItem = NSMenuItem(title: "Theme", action: nil, keyEquivalent: "")
        themeItem.submenu = themeMenu
        menu.addItem(themeItem)

        let levels: [(String, CGFloat)] = [
            ("Opaque", 0), ("Frosted — 10%", 0.10), ("Soft — 22% (default)", 0.22),
            ("Glass — 35%", 0.35), ("Clear — 50%", 0.50), ("Ghost — 70%", 0.70),
        ]
        let tMenu = NSMenu(title: "Transparency")
        for (scope, label) in scopes {
            let lm = NSMenu(title: label)
            let primary = themeRoles(for: scope, window: w).first ?? .notepad
            let alpha = (w.themeColor(primary).usingColorSpace(.sRGB) ?? w.themeColor(primary)).alphaComponent
            for (name, t) in levels {
                lm.addItem(menuItem(name, state: abs((1 - t) - alpha) < 0.03) { [weak self, weak w] in
                    guard let self, let w else { return }
                    self.applyTransparency(t, scope: scope, to: w, section: section)
                })
            }
            let li = NSMenuItem(title: label.replacingOccurrences(of: " Only", with: ""),
                                action: nil, keyEquivalent: "")
            li.submenu = lm
            tMenu.addItem(li)
        }
        let tItem = NSMenuItem(title: "Transparency", action: nil, keyEquivalent: "")
        tItem.submenu = tMenu
        menu.addItem(tItem)
    }

    func headerStyleMenuItem() -> NSMenuItem {
        let sub = NSMenu(title: "Header Style")
        let original = HeaderStyle.current
        let preview = HeaderStylePreviewDelegate(original: original)
        sub.delegate = preview
        headerStylePreviewDelegate = preview
        for (i, style) in HeaderStyle.allCases.enumerated() {
            let item = menuItem(style.label, state: style == original) { [weak self] in
                preview.committed = true
                self?.setHeaderStyle(style)
            }
            item.tag = i
            sub.addItem(item)
        }
        let item = NSMenuItem(title: "Header Style", action: nil, keyEquivalent: "")
        item.submenu = sub
        return item
    }

    func setHeaderStyle(_ style: HeaderStyle) {
        HeaderStyle.current = style
        saveConfigValue(section: "app", key: "header-style", value: style.rawValue)
        log("[app] header-style = \(style.rawValue)")
    }

    private func themeRoles(for scope: ThemeScope, window w: PopupWindow) -> [PopupWindow.ThemeRole] {
        switch scope {
        case .window:
            var r: [PopupWindow.ThemeRole] = [.notepad, .header]
            if w.hasFileBrowser { r.append(.browser) }
            if w.hasTerminalDrawer { r.append(.terminal) }
            return w.config.editMode ? r : [.browser, .header, .notepad]
        case .notepad: return [.notepad, .header]
        case .terminal: return [.terminal]
        case .browser: return [.browser]
        }
    }

    private func configKey(for role: PopupWindow.ThemeRole) -> String {
        switch role {
        case .browser: return "browser-background"
        case .terminal: return "terminal-background"
        case .notepad: return "background-color"
        case .header: return "header-color"
        }
    }

    private func updateSpecColors(section: String, _ kv: [(String, NSColor?)]) {
        guard let i = commands.firstIndex(where: { $0.name == section }) else { return }
        for (key, c) in kv {
            switch key {
            case "browser-background": commands[i].browserBackground = c
            case "terminal-background": commands[i].terminalBackground = c
            case "background-color": commands[i].backgroundColor = c
                if c != nil { commands[i].tintAlpha = nil }
            case "header-color": commands[i].headerColor = c
            case "text-color": commands[i].textColor = c
            case "dim-color": commands[i].dimColor = c
            case "highlight-color": commands[i].highlightColor = c
            case "accent-color": commands[i].accentColor = c
            case "terminal-foreground": commands[i].terminalForeground = c
            default: break
            }
        }
    }

    private func commitColors(_ w: PopupWindow, section: String, _ kv: [(String, NSColor?)]) {
        updateSpecColors(section: section, kv)
        saveConfigValues(section: section, kv.map { ($0.0, $0.1.map(hexString)) })
        log("commands.toml [\(section)]: " + kv.map { "\($0.0)=\($0.1.map(hexString) ?? "-")" }.joined(separator: " "))
    }

    func applyThemePreset(_ p: ThemePreset, scope: ThemeScope,
                          to w: PopupWindow, section: String, persist: Bool = true) {
        var kv: [(String, NSColor?)] = []
        func paint(_ role: PopupWindow.ThemeRole, _ c: NSColor) {
            let cur = w.themeColor(role).usingColorSpace(.sRGB) ?? w.themeColor(role)
            let alpha = max(cur.alphaComponent, presetMinOpacity)
            let col = (c.usingColorSpace(.sRGB) ?? c).withAlphaComponent(alpha)
            w.setThemeColor(col, for: role)
            kv.append((configKey(for: role), col))
        }
        switch scope {
        case .window:
            paint(.notepad, p.background)
            paint(.header, p.header)
            if w.hasFileBrowser { paint(.browser, w.config.editMode ? p.browser : p.background) }
            if w.hasTerminalDrawer { paint(.terminal, p.terminal) }
            w.setTerminalForeground(nil)
            kv.append(("terminal-foreground", nil))
        case .notepad:
            paint(.notepad, p.background)
            paint(.header, p.header)
        case .terminal:
            paint(.terminal, p.background)
            w.setTerminalForeground(p.text)
            kv.append(("terminal-foreground", p.text))
        case .browser:
            paint(.browser, p.background)
        }
        if scope == .window || scope == .notepad {
            var probe = w.config.colors
            probe.accent = p.accent
            probe.text = p.text
            w.setTextColors(text: p.text, dim: p.dim, highlight: p.highlight, accent: p.accent,
                            palette: p.palette, border: probe.outline)
            kv += [("text-color", p.text), ("dim-color", p.dim), ("highlight-color", p.highlight),
                   ("accent-color", p.accent)]
        }
        guard persist else { return }
        commitColors(w, section: section, kv)
        if scope == .window || scope == .notepad {
            if let i = commands.firstIndex(where: { $0.name == section }) { commands[i].palette = p.palette }
            saveConfigValue(section: section, key: "palette", value: paletteString(p.palette))
        }
        log("theme '\(p.name)' applied to [\(section)] scope=\(scope)")
    }

    func applyTransparency(_ t: CGFloat, scope: ThemeScope,
                           to w: PopupWindow, section: String) {
        var kv: [(String, NSColor?)] = []
        for role in themeRoles(for: scope, window: w) {
            let cur = w.themeColor(role).usingColorSpace(.sRGB) ?? w.themeColor(role)
            let col = cur.withAlphaComponent(max(1 - t, 0.08))
            w.setThemeColor(col, for: role)
            kv.append((configKey(for: role), col))
        }
        commitColors(w, section: section, kv)
    }

    func resetWindowTheme(_ w: PopupWindow, section: String) {
        removeColorKeysFromConfig(section: section)
        updateSpecColors(section: section, [
            "browser-background", "terminal-background", "background-color", "header-color",
            "text-color", "dim-color", "highlight-color", "accent-color", "terminal-foreground",
        ].map { ($0, nil) })
        let base = PopupConfig(name: "")
        w.setThemeColor(THEME_BROWSER ?? base.fileBrowserBackground, for: .browser)
        w.setThemeColor(THEME_TERMINAL ?? base.terminalBackground, for: .terminal)
        w.setThemeColor(BAR.withAlphaComponent(base.tintAlpha), for: .notepad)
        w.setThemeColor(headerBlueSilver, for: .header)
        w.setTerminalForeground(nil)
        if let i = commands.firstIndex(where: { $0.name == section }) { commands[i].palette = nil }
        let base0 = windowColors()
        w.setTextColors(text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT,
                        palette: THEME_PALETTE, border: base0.border)
        NSColorPanel.shared.orderOut(nil)
        log("theme reset for [\(section)] — back to system defaults")
    }

    func addCardThemeMenu(to menu: NSMenu, view: SlotView) {
        let section: String
        switch view {
        case .confluence: section = "confluence"
        case .ai: section = "ai"
        case .compare, .compareText: section = "compare"
        default: return
        }
        let themeMenu = NSMenu(title: "Theme")
        let presets = ThemePreset.all()
        let toneTitles: [ThemePreset.Tone: String] = [.mid: "Mid Tones", .dark: "Dark", .light: "Light"]
        let preview = ThemePreviewDelegate(
            onHighlight: { [weak self] tag in
                guard let self else { return }
                let p = tag.flatMap { presets.indices.contains($0) ? presets[$0] : nil }
                guard cardThemeOverride[section]?.name != p?.name else { return }
                cardThemeOverride[section] = p
                self.rebuildCard(view)
            },
            onClose: { [weak self] in
                guard let self, cardThemeOverride[section] != nil else { return }
                cardThemeOverride[section] = nil
                self.rebuildCard(view)
            })
        themeMenu.delegate = preview
        themePreviewDelegate = preview
        let order = presets.indices.sorted {
            let a = presets[$0], b = presets[$1]
            if a.tone != b.tone { return a.tone.rawValue < b.tone.rawValue }
            return a.background.relativeLuminance > b.background.relativeLuminance
        }
        var lastTone: ThemePreset.Tone?
        for i in order {
            let p = presets[i]
            if p.tone != lastTone {
                if lastTone != nil { themeMenu.addItem(.separator()) }
                themeMenu.addItem(.sectionHeader(title: toneTitles[p.tone] ?? ""))
                lastTone = p.tone
            }
            let item = menuItem(p.name) { [weak self] in
                preview.committed = true
                cardThemeOverride[section] = nil
                self?.applyCardTheme(p, section: section, view: view)
            }
            item.tag = i
            item.representedObject = i
            item.image = p.swatch()
            themeMenu.addItem(item)
        }
        themeMenu.addItem(.separator())
        themeMenu.addItem(menuItem("Reset to Defaults") { [weak self] in
            preview.committed = true
            cardThemeOverride[section] = nil
            self?.applyCardTheme(nil, section: section, view: view)
        })
        let item = NSMenuItem(title: "Theme", action: nil, keyEquivalent: "")
        item.submenu = themeMenu
        menu.addItem(item)
    }

    private func applyCardTheme(_ p: ThemePreset?, section: String, view: SlotView) {
        var kv: [(String, String?)]
        if let p {
            kv = [("background-color", hexString(p.background.withAlphaComponent(1))),
                  ("header-color", hexString(p.header)),
                  ("text-color", hexString(p.text)), ("dim-color", hexString(p.dim)),
                  ("highlight-color", hexString(p.highlight)), ("accent-color", hexString(p.accent)),
                  ("palette", paletteString(p.palette))]
        } else {
            kv = ["background-color", "header-color", "text-color", "dim-color",
                  "highlight-color", "accent-color", "palette"].map { ($0, nil) }
        }
        saveConfigValues(section: section, kv)
        log("card theme '\(p?.name ?? "reset")' applied to [\(section)]")
        rebuildCard(view)
    }

    func rebuildCard(_ view: SlotView) {
        let v: SlotView = view == .compareText ? .compare : view
        let wasCurrent = slot.current == view || slot.current == v
        let f = slot.currentFrame()
        let w: CardWindowController?
        switch v {
        case .confluence: w = ConfluenceWindow.current
        case .ai: w = AIWindow.current
        default: w = CompareWindow.current
        }
        (w as? CompareWindow)?.persistNow()
        if view == .compareText, let sub = CompareWindow.sub { sub.leaveWindow() }
        w?.leaveWindow()
        switch v {
        case .confluence: ConfluenceWindow.discard()
        case .ai: AIWindow.discard()
        default: CompareWindow.discard()
        }
        if wasCurrent, ensureSlotMember(v, frame: f) { slot.present(v) }
    }

    func addGlobalWindowItems(to parent: NSMenu) {
        let menu = NSMenu(title: "Global Window Options")
        menu.autoenablesItems = false
        let group = NSMenuItem(title: "Global Window Options", action: nil, keyEquivalent: "")
        group.submenu = menu
        parent.addItem(group)
        let hide = menuItem("Hide When Focus Is Lost", state: settings.hideOnFocusLoss) { [weak self] in
            self?.setGlobalHideOnFocusLoss(!settings.hideOnFocusLoss)
        }
        hide.toolTip = "On: the window hides when you switch to another app. Off: it stays until ✕, Cmd+W, the hotkey or Esc (when the view's \"Esc Hides Window\" is on)"
        menu.addItem(hide)
        menu.addItem(headerStyleMenuItem())
    }

    func setGlobalHideOnFocusLoss(_ on: Bool) {
        settings.hideOnFocusLoss = on
        saveConfigValue(section: "app", key: "hide-on-focus-loss", value: on ? "true" : "false")
        for v in Self.sharedViews {
            guard let p = slotMember(v) as? PopupWindow, p.config.sticky else { continue }
            p.config.sticky = false
            if let i = commands.firstIndex(where: { $0.windowName == p.config.name }) {
                commands[i].sticky = false
                removeConfigValue(section: commands[i].name, key: "sticky")
            }
        }
        log("[app] hide-on-focus-loss = \(on)")
    }

    static let sharedViews: [SlotView] = [.notes, .files, .jira, .detail, .releases, .confluence, .ai, .compare]

    func shortcutsMenuItem(for w: PopupWindow, view: String) -> NSMenuItem {
        let item = menuItem("Keyboard Shortcuts…") { [weak self, weak w] in
            guard let self, let w else { return }
            self.showShortcuts(on: w, view: view)
        }
        item.keyEquivalent = "/"
        item.keyEquivalentModifierMask = .command
        return item
    }

    func showShortcuts(on w: PopupWindow, view: String) {
        let titles = ["notes": "Notes", "files": "Files", "jira": "Jira"]
        func items(_ v: String) -> [(keys: String, what: String)] {
            shortcutEntries.filter { $0.view == v }.map { ($0.keys, $0.what) }
        }
        var groups: [PopupWindow.ShortcutGroup] = []
        if let t = titles[view], !items(view).isEmpty { groups.append((t, items(view))) }
        groups += sharedShortcutGroups()
        guard !groups.isEmpty else {
            w.showToast("No shortcuts listed — add a [shortcuts] section to commands.toml",
                        symbol: "keyboard")
            return
        }
        w.showShortcuts(groups)
    }

    private func presentThemeRoleMenu(for w: PopupWindow,
                                      roles: [PopupWindow.ThemeRole],
                                      section: String) {
        let menu = NSMenu(title: "Pick a color for…")
        for role in roles {
            var enabled = true
            switch role {
            case .terminal:
                enabled = w.terminalShown
            case .browser:
                enabled = w.fileBrowserShown
            case .notepad, .header:
                break
            }
            let item: NSMenuItem
            if enabled {
                item = NSMenuItem(title: role.label, action: #selector(pickThemeRole(_:)), keyEquivalent: "")
            } else {
                let attr = NSAttributedString(
                    string: role.label,
                    attributes: [
                        .foregroundColor: NSColor.systemGray,
                        .strikethroughStyle: NSUnderlineStyle.single.rawValue,
                    ])
                item = NSMenuItem(title: "", action: nil, keyEquivalent: "")
                item.attributedTitle = attr
            }
            item.target = self
            item.representedObject = role
            item.isEnabled = enabled
            menu.addItem(item)
            log("menu role=\(role.rawValue) enabled=\(enabled) terminalShown=\(w.terminalShown) fileBrowserShown=\(w.fileBrowserShown)")
        }
        menu.addItem(.separator())
        let reset = NSMenuItem(title: "Reset to system defaults",
                               action: #selector(resetThemeColors(_:)),
                               keyEquivalent: "")
        reset.target = self
        menu.addItem(reset)
        pickerWindow = w
        pickerSection = section
        if let cv = w.nativeWindow.contentView, let r = w.headerButtonRect(60) {
            menu.popUp(positioning: nil, at: NSPoint(x: r.midX, y: r.minY), in: cv)
        } else if let cv = w.nativeWindow.contentView {
            menu.popUp(positioning: nil, at: cv.convert(NSEvent.mouseLocation, from: nil), in: cv)
        }
    }

    @objc private func pickThemeRole(_ sender: NSMenuItem) {
        guard let role = sender.representedObject as? PopupWindow.ThemeRole,
              let w = pickerWindow else { return }
        startColorPicker(for: w, role: role)
    }

    @objc private func resetThemeColors(_ sender: Any?) {
        guard let w = pickerWindow else { return }
        resetWindowTheme(w, section: pickerSection)
    }

    private func startColorPicker(for w: PopupWindow, role: PopupWindow.ThemeRole) {
        pickerWindow = w
        pickerRole = role
        pickerOriginal = w.themeColor(role)
        pickerCommitted = false
        pickerSawVisible = false
        let seed = (pickerOriginal ?? .clear).usingColorSpace(.sRGB) ?? .clear
        pickerHue = seed.withAlphaComponent(1)
        pickerTransparency = 1.0 - seed.alphaComponent
        let panel = NSColorPanel.shared
        panel.mode = .wheel
        panel.color = pickerHue
        panel.showsAlpha = false
        panel.isContinuous = true
        panel.setTarget(self)
        panel.setAction(#selector(panelColorChanged(_:)))
        let acc = NSView(frame: NSRect(x: 0, y: 0, width: 260, height: 120))
        let hexLabel = NSTextField(labelWithString: "")
        hexLabel.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .medium)
        hexLabel.alignment = .center
        hexLabel.frame = NSRect(x: 0, y: 98, width: 260, height: 16)
        pickerHexLabel = hexLabel
        let tLab = NSTextField(labelWithString: "")
        tLab.font = NSFont.systemFont(ofSize: 11)
        tLab.textColor = .secondaryLabelColor
        tLab.alignment = .center
        tLab.frame = NSRect(x: 0, y: 78, width: 260, height: 16)
        pickerTransparencyLabel = tLab
        let slider = NSSlider(value: Double(pickerTransparency * 100), minValue: 0, maxValue: 100,
                              target: self,
                              action: #selector(pickerTransparencyChanged(_:)))
        slider.isContinuous = true
        slider.frame = NSRect(x: 12, y: 56, width: 236, height: 18)
        let applyButton = NSButton(title: "Apply", target: self,
                                   action: #selector(applyPickerColor(_:)))
        applyButton.keyEquivalent = "\r"
        applyButton.bezelStyle = .rounded
        applyButton.frame = NSRect(x: 0, y: 2, width: 124, height: 26)
        let cancelButton = NSButton(title: "Cancel", target: self,
                                    action: #selector(cancelPickerColor(_:)))
        cancelButton.bezelStyle = .rounded
        cancelButton.frame = NSRect(x: 136, y: 2, width: 124, height: 26)
        acc.addSubview(hexLabel)
        acc.addSubview(tLab)
        acc.addSubview(slider)
        acc.addSubview(applyButton)
        acc.addSubview(cancelButton)
        panel.accessoryView = acc
        if let o = pickerPanelObserver {
            NotificationCenter.default.removeObserver(o)
        }
        pickerPanelObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification, object: panel, queue: .main
        ) { [weak self] _ in
            self?.revertPickerIfCancelled()
        }
        pickerWatchdog?.invalidate()
        let watchdog = Timer(timeInterval: 0.25, repeats: true) { [weak self] _ in
            guard let self else { return }
            let p = NSColorPanel.shared
            if p.isVisible {
                self.pickerSawVisible = true
            } else if self.pickerSawVisible {
                self.pickerWatchdog?.invalidate()
                self.pickerWatchdog = nil
                if let o = self.pickerPanelObserver {
                    NotificationCenter.default.removeObserver(o)
                    self.pickerPanelObserver = nil
                }
                if !self.pickerCommitted {
                    self.revertPicker()
                }
            }
        }
        pickerWatchdog = watchdog
        RunLoop.main.add(watchdog, forMode: .common)
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        applyPickerPreview()
        let hex = hexString(seed)
        log("color picker opened for [\(pickerSection)] \(role.rawValue) seed=#\(hex) trans=\(Int(pickerTransparency*100))% terminalShown=\(w.terminalShown) browserShown=\(w.fileBrowserShown)")
    }

    @objc private func panelColorChanged(_ sender: Any?) {
        guard pickerWindow != nil, pickerRole != nil else { return }
        let raw = NSColorPanel.shared.color
        pickerHue = (raw.usingColorSpace(.sRGB) ?? raw).withAlphaComponent(1)
        applyPickerPreview()
    }

    @objc private func pickerTransparencyChanged(_ sender: NSSlider) {
        pickerTransparency = CGFloat(sender.doubleValue) / 100.0
        applyPickerPreview()
    }

    private func applyPickerPreview() {
        guard let w = pickerWindow, let role = pickerRole else { return }
        let alpha = max(1.0 - pickerTransparency, 0.08)
        let color = pickerHue.withAlphaComponent(alpha)
        w.setThemeColor(color, for: role)
        let hex = hexString(color)
        let pct = Int((pickerTransparency * 100).rounded())
        pickerHexLabel?.stringValue = "#\(hex)"
        pickerTransparencyLabel?.stringValue = "Transparency: \(pct)%"
    }

    @objc private func applyPickerColor(_ sender: Any?) {
        pickerCommitted = true
        persistPickerColor()
        NSColorPanel.shared.close()
    }

    @objc private func cancelPickerColor(_ sender: Any?) {
        revertPicker()
        NSColorPanel.shared.close()
    }

    private func revertPickerIfCancelled() {
        guard !pickerCommitted else { return }
        revertPicker()
    }

    private func revertPicker() {
        guard let w = pickerWindow, let role = pickerRole,
              let original = pickerOriginal else { return }
        w.setThemeColor(original, for: role)
        log("color picker cancelled for [\(pickerSection)] — reverted")
    }

    private func dismissPickerIfOpen(for w: PopupWindow) {
        guard pickerWindow === w else { return }
        pickerWatchdog?.invalidate()
        pickerWatchdog = nil
        if let o = pickerPanelObserver {
            NotificationCenter.default.removeObserver(o)
            pickerPanelObserver = nil
        }
        if !pickerCommitted {
            revertPicker()
        }
        pickerWindow = nil
        pickerRole = nil
        pickerOriginal = nil
        NSColorPanel.shared.orderOut(nil)
        log("color picker dismissed — window hidden")
    }

    private func persistPickerColor() {
        guard pickerWindow != nil, pickerRole != nil else { return }
        let alpha = max(1.0 - pickerTransparency, 0.08)
        let color = pickerHue.withAlphaComponent(alpha)
        let hex = hexString(color)
        guard !hex.isEmpty, !pickerSection.isEmpty else { return }
        let key: String
        switch pickerRole! {
        case .browser: key = "browser-background"
        case .terminal: key = "terminal-background"
        case .notepad: key = "background-color"
        case .header: key = "header-color"
        }
        saveConfigValue(section: pickerSection, key: key, value: hex)
        updateSpecColors(section: pickerSection, [(key, color)])
        log("commands.toml [\(pickerSection)]: \(key) -> #\(hex)")
    }

    private func hexString(_ c: NSColor) -> String {
        let cc = c.usingColorSpace(.sRGB) ?? c
        let r = Int(round(cc.redComponent * 255))
        let g = Int(round(cc.greenComponent * 255))
        let b = Int(round(cc.blueComponent * 255))
        let a = Int(round(cc.alphaComponent * 255))
        return a < 255
            ? String(format: "%02X%02X%02X%02X", a, r, g, b)
            : String(format: "%02X%02X%02X", r, g, b)
    }

    private func removeColorKeysFromConfig(section: String) {
        let confPath = settings.commandsConfPath
        guard let content = readConfigText() else {
            log("commands.toml: cannot read \(confPath)")
            return
        }
        let keys: Set<String> = ["header-color", "background-color",
                                 "browser-background", "terminal-background",
                                 "tint-alpha", "text-color", "dim-color",
                                 "highlight-color", "accent-color", "terminal-foreground",
                                 "palette"]
        var lines = configLines(content)
        let overrides = configSectionEntries(lines, section).filter { keys.contains($0.key) }
        guard !overrides.isEmpty else {
            log("commands.toml [\(section)]: no color overrides to reset")
            return
        }
        for e in overrides.reversed() { lines.remove(at: e.index) }
        writeConfigText(lines.joined(separator: "\n"))
        log("commands.toml [\(section)]: reset color overrides to defaults")
    }

    private func openListWindow(_ cmd: CommandSpec,
                                restoreWID: String?, restorePID: pid_t?) {
        guard !cmd.sources.isEmpty else {
            log("list '\(cmd.name)': no source configured")
            return
        }
        let tabs: [(path: String, items: [FieldRow])] =
            expandPaths(cmd.sources, extensions: ["json", "tsv"]).map { path in
                return (path, loadListItems(path, cmd: cmd, columns: ListSession.tabColumns(cmd, path)))
            }
        let columns = ListSession.tabColumns(cmd, tabs.first?.path)
        let cfg = listWindowConfig(cmd, tabCount: tabs.count, columns: columns)
        let w = PopupWindow(config: cfg)
        if ListSession.isJira(cmd) { w.textZoomKey = "textZoom.jira" }
        let session = ListSession(host: self, cmd: cmd, window: w, tabs: tabs, columns: columns,
                                  restoreWID: restoreWID, restorePID: restorePID)
        session.install()
        subWindows.append(w)
        w.tabFooterText = ""
        w.onSidebarWidthChange = { width in
            saveConfigValue(section: cmd.name, key: "sidebar-width", value: String(Int(width)))
        }
        w.tabRowIcon = { [weak w] i in
            guard let w, w.tabTitles.indices.contains(i) else { return nil }
            let name = w.tabTitles[i].lowercased()
            if name.contains("blacklist_release") { return "eye.slash" }
            if name.hasPrefix("release") { return "shippingbox" }
            if name.hasPrefix("favorites") { return "star" }
            if name.hasPrefix("search") { return "magnifyingglass" }
            return "list.bullet.rectangle"
        }
        if ListSession.isJira(cmd) {
            w.tabRowTitle = { [weak w] i in
                guard let w, w.tabTitles.indices.contains(i) else { return nil }
                return JiraPoll.listTitle(file: w.tabTitles[i])
            }
            w.quietOKTabBadges = true
        }
        placeSlotWindow(w)
        w.quietShow = slotPrewarming
        w.show()
        session.didShow()
    }

    private func listWindowConfig(_ cmd: CommandSpec, tabCount: Int,
                                  columns: [ListColumn]) -> PopupConfig {
        let isJira = ListSession.isJira(cmd)
        var cfg = PopupConfig(name: cmd.windowName)
        cfg.enableToggle = false
        cfg.enableResize = cmd.resize
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.floating = isJira ? false : (cmd.float ?? false)
        cfg.escCloseCount = ListSession.inSlot(cmd) ? 1 : max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
        cfg.wrapContent = true
        cfg.showSearchBar = true
        cfg.dragHeader = true
        cfg.tabs = tabCount > 1
        if isJira {
            cfg.tabsSidebarWidth = cmd.sidebarWidth
            cfg.tabsSidebarTitle = "Synced"
            cfg.inspectorWidth = cmd.inspectorWidth
        }
        cfg.opaqueTabs = cmd.tabsOpaque ?? true
        cfg.scrollableRows = true
        cfg.dynamicHeight = false
        cfg.clickToSelect = true
        cfg.wrapNavigation = false
        cfg.highlightMatches = true
        cfg.filters = !cmd.filters.isEmpty
        cfg.selectableRows = cmd.checkbox ?? !cmd.copyFields.isEmpty
        cfg.rowStars = isJira
        cfg.copyRowsButton = !isJira
        cfg.bodyMaxLines = cmd.bodyLines > 0 ? cmd.bodyLines : 5
        cfg.height = cmd.height > 0 ? cmd.height : defaultListSize.height
        cfg.width = cmd.width > 0 ? cmd.width : defaultListSize.width
        cfg.headerHeight = 30
        cfg.titlePill = false
        applyWindowTheme(&cfg, cmd)
        if cmd.searchWidth > 0 { cfg.searchWidthFraction = cmd.searchWidth }
        if cmd.maxStretch > 0 { cfg.maxRowStretch = cmd.maxStretch }
        if !columns.isEmpty {
            cfg.tableColumns = columns.map { $0.popup }
            cfg.rowHeight = cmd.tableRowHeight > 0 ? cmd.tableRowHeight : 32
        }
        cfg.tableCellStyle = jiraCellStyle
        return cfg
    }

    private func applyBrowserSettings(_ cfg: inout PopupConfig) {
        guard let cmd = commands.first(where: { $0.kind == .files }) else { return }
        if let v = cmd.sort { cfg.browserSort = v }
        if let v = cmd.sortDescending { cfg.browserSortDescending = v }
        if let v = cmd.searchLimit, v > 0 { cfg.browserSearchLimit = v }
        if let v = cmd.searchExclude { cfg.browserSearchExcludes = v }
        if let v = cmd.terminalWords, !v.isEmpty { cfg.browserTerminalWords = v }
    }
    private func saveBrowserSort(_ key: String, _ desc: Bool) {
        let section = commands.first(where: { $0.kind == .files })?.name ?? "files"
        if let i = commands.firstIndex(where: { $0.kind == .files }) {
            commands[i].sort = key
            commands[i].sortDescending = desc
        }
        saveConfigValues(section: section, [("sort", key), ("sort-order", desc ? "desc" : "asc")])
    }
    private func openInTerminalApp(_ dir: String) {
        var app = settings.terminalApp
        if app.isEmpty {
            let ghostty = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.mitchellh.ghostty")
            app = ghostty != nil ? "Ghostty" : "Terminal"
        }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        p.arguments = ["-a", app, dir]
        do { try p.run() } catch { log("terminal-app \(app): \(error)") }
        log("opened \(app) in \(dir)")
    }

    private func fileBrowserFavorites() -> [String] {
        commands.first(where: { $0.kind == .files })?.favorites ?? []
    }

    private var recentObservers: [NSObjectProtocol] = []
    func attachRecent(_ fb: PopupFileBrowser) {
        guard RecentFiles.shared.enabled else { return }
        let everywhere = filesCommand?.recentEverywhere ?? true
        fb.virtualLists = [
            .init(title: "Recent", symbol: "clock",
                  status: "newest first · created or changed anywhere "
                      + (everywhere ? "on this Mac (outside system folders)" : "in ~ or /tmp"),
                  provider: { RecentFiles.shared.entries().map { ($0.path, $0.at, $0.source) } }),
            .init(title: "Arrived", symbol: "arrow.down.circle",
                  status: "downloads, AirDrop, Messages / Mail saves — wherever they were saved",
                  provider: { RecentFiles.shared.entries(arrivedOnly: true).map { ($0.path, $0.at, $0.source) } }),
        ]
        let o = NotificationCenter.default.addObserver(forName: RecentFiles.changed, object: nil,
                                                       queue: .main) { [weak fb] _ in
            fb?.recentChanged()
        }
        recentObservers.append(o)
    }

    private func makeFileBrowser(_ cfg: PopupConfig, startDir: String, in w: PopupWindow,
                                 tag: String, opened: String) -> PopupFileBrowser {
        var browserCfg = cfg
        applyBrowserSettings(&browserCfg)
        let fb = PopupFileBrowser(config: browserCfg, startDir: startDir,
                                  staticFavorites: fileBrowserFavorites())
        fb.onOpen = { [weak self, weak w] path in
            self?.log("\(tag): \(opened) \(path)")
            FilePopup.open(path, over: w?.nativeWindow)
        }
        fb.onCopyPath = { [weak self] p in
            self?.copy(p, "path: \(p)")
            PathShelf.shared.add([p], why: .copied)
        }
        fb.onSortChange = { [weak self] key, desc in self?.saveBrowserSort(key, desc) }
        w.onOpenExternalTerminal = { [weak self] dir in self?.openInTerminalApp(dir) }
        fb.onStatus = { [weak self] s in
            if !s.isEmpty { self?.log("\(tag): \(s)") }
        }
        w.onFileBrowserOpenInNotes = { [weak self] p in
            self?.openNoteFile(p)
        }
        attachRecent(fb)
        return fb
    }

    func configureRecentFiles() {
        let cmd = commands.first(where: { $0.kind == .files })
        FileDrag.onFileOp = { RecentFiles.shared.ownChange(from: $0, to: $1) }
        FileListPane.onCompare = compareEnabled() ? { [weak self] a, b in self?.showCompare([a, b]) } : nil
        RecentFiles.shared.configure(enabled: cmd?.recent ?? true, days: cmd?.recentDays ?? 7,
                                     limit: cmd?.recentLimit ?? 200, excludes: cmd?.recentExclude ?? [],
                                     everywhere: cmd?.recentEverywhere ?? true)
        configurePathShelf()
    }

    var pathsCommand: CommandSpec? { commands.first { $0.name == "paths" } }

    func configurePathShelf() {
        guard pathsCommand != nil else {
            RecentFiles.shared.onKept = nil
            RecentFiles.shared.onRenamed = nil
            FileDrag.onDragOut = nil
            clipboardPaths?.stop()
            pathsWindow?.hide()
            return
        }
        let shelf = PathShelf.shared
        let limit = configSectionValue("paths", "limit").flatMap { Int($0) } ?? PathShelf.maxLimit
        let ignore = configSectionValue("paths", "ignore-file").map { ($0 as NSString).expandingTildeInPath }
            .flatMap { $0.isEmpty ? nil : ($0.hasPrefix("/") ? $0 : userDir + "/" + $0) }
            ?? userDir + "/config/paths.ignore"
        shelf.configure(limit: limit, ignoreFile: ignore)
        RecentFiles.shared.onKept = { shelf.observe($0, created: $1, origin: $2) }
        RecentFiles.shared.onRenamed = { shelf.renamed(from: $0, to: $1) }
        FileDrag.onDragOut = { shelf.add($0, why: .copied) }
        if shelf.isEmpty, pathsSeedObserver == nil {
            pathsSeedObserver = NotificationCenter.default.addObserver(
                forName: RecentFiles.changed, object: nil, queue: .main) { [weak self] _ in
                guard let self, let o = self.pathsSeedObserver else { return }
                NotificationCenter.default.removeObserver(o)
                self.pathsSeedObserver = nil
                shelf.seed(from: RecentFiles.shared.entries())
            }
        }
        let watchClipboard = !["false", "no", "0", "off"].contains((configSectionValue("paths", "clipboard") ?? "").lowercased())
        if watchClipboard {
            if clipboardPaths == nil {
                let c = ClipboardPaths()
                c.onPaths = { shelf.add($0, why: .clipboard) }
                clipboardPaths = c
            }
            clipboardPaths?.start()
        } else {
            clipboardPaths?.stop()
        }
    }

    func showNoteFind(grep: Bool = false) {
        let mine = grep ? noteGrepWindow : noteFindWindow
        if mine?.isShown == true { mine?.hide(); return }
        guard let notes = noteWindow, slot.current == .notes else { return }
        (grep ? noteFindWindow : noteGrepWindow)?.hide()
        let f = mine ?? {
            let n = NoteFindWindow(commands.first(where: { $0.kind == .note }), mode: grep ? .grep : .files)
            n.onOpen = { [weak self] path, line in
                self?.openNoteFile(path)
                if let line {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                        self?.noteWindow?.vimCommand("normal! \(line)Gzz")
                    }
                }
            }
            n.log = { [weak self] in self?.log($0) }
            if grep { noteGrepWindow = n } else { noteFindWindow = n }
            subWindows.append(n.window)
            return n
        }()
        f.openPaths = { notes.openNotePaths?() ?? [] }
        f.show()
        reclaimToolKey(f.window) { [weak f] in f?.window.focusSearchField() }
    }

    func showSidebarJump(_ w: PopupWindow) {
        if sidebarJumpWindow?.isShown == true { sidebarJumpWindow?.hide(); return }
        let f = sidebarJumpWindow ?? {
            let n = SidebarJumpWindow(commands.first(where: { $0.name == "jira" }))
            sidebarJumpWindow = n
            subWindows.append(n.window)
            return n
        }()
        f.onJump = { [weak w] row in w?.sidebarJump(row) }
        f.show(items: w.sidebarJumpItems())
        reclaimToolKey(f.window) { [weak f] in f?.window.focusSearchField() }
    }

    func showPaths(_ cmd: CommandSpec) {
        let ret = (configSectionValue("paths", "return") ?? "file").trimmingCharacters(in: .whitespaces).lowercased()
        if pathsWindow == nil {
            let p = PathsWindow(cmd, returnAction: ["file", "path", "open"].contains(ret) ? ret : "file")
            p.onOpenInNotes = { [weak self] path in self?.openNoteFile(path) }
            p.onOpenTerminal = { [weak self] dir in self?.openInTerminalApp(dir) }
            p.onCopied = { [weak self] in self?.clipboardPaths?.ownWrite() }
            p.log = { [weak self] in self?.log($0) }
            pathsWindow = p
            subWindows.append(p.window)
        }
        pathsWindow?.show()
        if let w = pathsWindow?.window {
            reclaimToolKey(w) { [weak w] in w?.focusSearchField() }
        }
        log("paths window opened")
    }

    private func openFilesWindow(_ cmd: CommandSpec,
                                 restoreWID: String?, restorePID: pid_t?) {
        let root = ((cmd.root ?? "~") as NSString).expandingTildeInPath
        guard FileManager.default.fileExists(atPath: root) else {
            log("files '\(cmd.name)': root \(root) does not exist")
            return
        }
        var cfg = PopupConfig(name: cmd.windowName)
        cfg.opaqueTabs = cmd.tabsOpaque ?? true
        cfg.enableToggle = false
        cfg.enableResize = cmd.resize
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.floating = false
        cfg.escCloseCount = settings.sharedWindow ? 1 : max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
        cfg.enableNavigation = false
        cfg.enableSearch = false
        cfg.showSearchBar = false
        cfg.dragHeader = true
        cfg.scrollableRows = true
        cfg.fileBrowserBackground = cmd.browserBackground
            ?? THEME_BROWSER ?? cfg.fileBrowserBackground
        cfg.terminalBackground = cmd.terminalBackground
            ?? THEME_TERMINAL ?? cfg.terminalBackground
        cfg.width = cmd.width > 0 ? cmd.width : 780
        cfg.height = cmd.height > 0 ? cmd.height : 560
        cfg.headerHeight = 30
        applyWindowTheme(&cfg, cmd)
        cfg.terminalForeground = cmd.terminalForeground
        let w = PopupWindow(config: cfg)
        w.headerIcon = settings.sharedWindow ? appIcon : filesNavIcon
        w.chromeHeaderTitle = nil
        w.copyPathButtonLabel = ""
        w.copyConfigButtonLabel = ""
        w.onChromeIconClick = { [weak self, weak w] in
            guard let self, let w else { return }
            let menu = NSMenu()
            menu.autoenablesItems = false
            self.addGlobalWindowItems(to: menu)
            menu.addItem(.separator())
            if settings.sharedWindow {
                menu.addItem(self.escHidesMenuItem(.files))
                menu.addItem(.separator())
            }
            self.addWindowSettingsItems(to: menu, window: w, section: cmd.name)
            menu.addItem(.separator())
            menu.addItem(self.openConfigMenuItem { [weak self] in self?.openNoteFile($0) })
            menu.addItem(self.shortcutsMenuItem(for: w, view: "files"))

            w.showHeaderMenu(menu)
        }
        w.onShowShortcuts = { [weak self, weak w] in
            guard let self, let w else { return }
            self.showShortcuts(on: w, view: "files")
        }

        let fb = makeFileBrowser(cfg, startDir: root, in: w,
                                 tag: "files '\(cmd.name)'", opened: "opened")
        w.installFileBrowser(fb, drawer: false)
        w.textZoomKey = "textZoom.files"
        fb.useSidebar(width: cmd.sidebarWidth)
        fb.onSidebarWidthChange = { width in
            saveConfigValue(section: cmd.name, key: "sidebar-width", value: String(Int(width)))
        }
        if cmd.startRecent { fb.showRecent() }

        w.onEscape = { [weak self] in
            if settings.sharedWindow { self?.slot.escapeAtTop(.files) } else { w.hide(restore: true) }
        }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.dismissPickerIfOpen(for: w)
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: restoreWID, restorePID: restorePID)
        }
        subWindows.append(w)
        placeSlotWindow(w)
        w.quietShow = slotPrewarming
        w.show()
        DispatchQueue.main.async { [weak w, weak fb] in
            if let w, let fb { w.nativeWindow.makeFirstResponder(fb.listView) }
        }
    }

    private func loadListItems(_ path: String, cmd: CommandSpec,
                               columns: [ListColumn]? = nil) -> [FieldRow] {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
              let text = String(data: data, encoding: .utf8) else {
            log("list '\(cmd.name)': cannot read \(path)")
            return []
        }
        var fields = cmd.filter.isEmpty
            ? [cmd.primary, cmd.content, cmd.trailing].compactMap { $0 }
            : cmd.filter
        if cmd.table {
            for c in (columns ?? cmd.columns) where c.filterable && !fields.contains(c.field) {
                fields.append(c.field)
            }
        }
        if let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] {
            return arr.compactMap { d in
                let str = { (k: String?) -> String? in
                    k.flatMap { d[$0] as? String }
                }
                guard let title = str(cmd.primary) else { return nil }
                let search = fields.compactMap { capped(str($0), 150) }.joined(separator: " ")
                var raw: [String: String] = [:]
                for (k, v) in d {
                    if let s = v as? String { raw[k] = s }
                }
                let detail = cmd.detail.map { spec -> String? in
                    let vals = spec.split(separator: ",")
                        .map { $0.trimmingCharacters(in: .whitespaces) }
                        .compactMap { (d[$0] as? String).map(compactTimestamp) }
                        .filter { !$0.isEmpty }
                    return vals.isEmpty ? nil : vals.joined(separator: " · ")
                }
                let trailing = cmd.trailing.map { spec -> String? in
                    let vals = spec.split(separator: ",")
                        .map { $0.trimmingCharacters(in: .whitespaces) }
                        .compactMap { (d[$0] as? String).map(compactTimestamp) }
                        .filter { !$0.isEmpty }
                    return vals.isEmpty ? nil : vals.joined(separator: " · ")
                }
                return FieldRow(title: title,
                                content: capped(str(cmd.content), cmd.contentCap),
                                trailing: trailing ?? str(cmd.trailing),
                                detail: detail ?? str(cmd.detail),
                                body: capped(str(cmd.body), 800),
                                searchText: search,
                                fields: raw)
            }
        }
        return text.split(separator: "\n").compactMap { line in
            let parts = line.split(separator: "\t").map(String.init)
            guard !parts.isEmpty else { return nil }
            let title = parts[0]
            let content = parts.count > 1 ? parts[1] : nil
            let trailing = parts.count > 2 ? parts[2] : nil
            let search = [title, content ?? "", trailing ?? ""].joined(separator: " ")
            return FieldRow(title: title,
                            content: capped(content, cmd.contentCap),
                            trailing: trailing,
                            detail: nil,
                            body: nil,
                            searchText: search,
                            fields: [:])
        }
    }

    private func capped(_ s: String?, _ cap: Int) -> String? {
        guard let s, cap > 0, s.count > cap else { return s }
        return String(s.prefix(cap)) + "…"
    }

    private func handleEscape() {
        if !popup.currentQuery.isEmpty {
            popup.clearInput()
            popup.setRows(filter(""))
            popup.selection = 0
        } else {
            popup.hide(restore: true)
        }
    }

    private func restoreFocus(_ restore: Bool) {
        if restore, let pid = savedPID {
            NSRunningApplication(processIdentifier: pid)?.activate(
                options: [.activateAllWindows])
        }
        if restore, let wid = savedWID {
            DispatchQueue.global(qos: .userInitiated).async {
                _ = aerospaceCall(["focus", "--window-id", wid])
            }
        }
        savedWID = nil
        savedPID = nil
    }
}

let authDebugPath = NSString(string: "~/.cache/ws-auth-debug").expandingTildeInPath

final class ServicesHandler: NSObject {
    private weak var controller: SwitcherController?
    init(_ controller: SwitcherController) {
        self.controller = controller
    }
    private func filePaths(from pboard: NSPasteboard) -> [String] {
        if let urls = pboard.readObjects(forClasses: [NSURL.self],
                                         options: [.urlReadingFileURLsOnly: true]) as? [URL] {
            let paths = urls.map { $0.path }
            if !paths.isEmpty { return paths }
        }
        if let files = pboard.propertyList(
            forType: NSPasteboard.PasteboardType("NSFilenamesPboardType")) as? [String] {
            return files
        }
        return []
    }
    @objc func copyFilePaths(_ pboard: NSPasteboard, userData: String, error: NSErrorPointer) {
        let paths = filePaths(from: pboard)
        guard !paths.isEmpty else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(paths.joined(separator: "\n"), forType: .string)
        ScreenToast.show(paths.count == 1 ? "Copied \((paths[0] as NSString).abbreviatingWithTildeInPath) to clipboard"
                                          : "Copied \(paths.count) paths to clipboard",
                         on: nil, symbol: "doc.on.clipboard")
    }
    @objc func openInNotes(_ pboard: NSPasteboard, userData: String, error: NSErrorPointer) {
        guard let first = filePaths(from: pboard).first else { return }
        controller?.openNoteFile(first)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    var controller: SwitcherController?
    let showOnLaunch: Bool
    let openCommand: String?
    private var servicesHandler: ServicesHandler?
    private var textKeyMonitor: Any?

    init(showOnLaunch: Bool, openCommand: String? = nil) {
        self.showOnLaunch = showOnLaunch
        self.openCommand = openCommand
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        PopupWindow.builtForHost = { [weak self] w in self?.controller?.slotView(of: w) != nil }
        installMainMenu()
        installTextKeys()
        installCrashHandler()
        if FileManager.default.fileExists(atPath: authDebugPath) {
            let mic = AVCaptureDevice.authorizationStatus(for: .audio).rawValue
            let speech = SFSpeechRecognizer.authorizationStatus().rawValue
            appendToFile(NSString(string: "~/.cache/ws-auth.log").expandingTildeInPath,
                         "auth-debug: mic=\(mic) speech=\(speech) bundle=\(Bundle.main.bundleIdentifier ?? "nil") launch=\(CommandLine.arguments[0])\n")
        }
        let c = SwitcherController()
        controller = c
        c.start()
        let sh = ServicesHandler(c)
        servicesHandler = sh
        NSApp.servicesProvider = sh
        NSUpdateDynamicServices()
        MenuTarget.controller = c
        installStatusMenus(c)
        c.configureRecentFiles()
        if showOnLaunch {
            c.show()
        }
        if let name = openCommand {
            if name == "notes" {
                c.showNotes()
            } else if name == "confluence" {
                c.showConfluence()
            } else if name == "ai" {
                c.showAI()
            } else if name == "compare" {
                c.showCompare()
            } else {
                c.showCommand(name)
            }
        }
        c.prewarmSlot()
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak c] in c?.screenshot.prewarm() }
        if AppInstall.requested || AppInstall.wantsSetupWindow {
            SetupWindow.show(controller: c)
        } else if !isRepoBuild, !showOnLaunch, openCommand == nil {
            c.showCommand("files")
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { controller?.showCommand("files") }
        return false
    }

    private func installStatusMenus(_ c: SwitcherController) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = utilityMenuGlyph
        item.button?.toolTip = "kitchen-sink"
        let menu = NSMenu()
        menu.delegate = MenuTarget.shared
        menu.autoenablesItems = false
        let issues = NSMenuItem(title: "Config Issues…",
                                action: #selector(MenuTarget.showConfigIssues(_:)),
                                keyEquivalent: "")
        issues.target = MenuTarget.shared
        issues.tag = MenuTarget.configIssuesTag
        issues.isHidden = configIssues.isEmpty
        menu.addItem(issues)
        let globalEnd = NSMenuItem.separator()
        globalEnd.tag = MenuTarget.globalGroupTag
        menu.addItem(globalEnd)

        addMenuItem(menu, "Reset Default Size", #selector(MenuTarget.resetWindowSize(_:)), key: "0", modifiers: .command)
        addMenuItem(menu, "Reset Default Colors", #selector(MenuTarget.resetWindowColors(_:)), key: "")
        menu.addItem(.separator())
        addMenuItem(menu, "Close Window", #selector(MenuTarget.closeWindow(_:)), key: "w", modifiers: .command)
        addMenuItem(menu, "Quit", #selector(MenuTarget.quitApp(_:)), key: "q", modifiers: .command)
        menu.addItem(.separator())
        addMenuItem(menu, "Setup & Health Check…", #selector(MenuTarget.openSetup(_:)), key: "")
        menu.addItem(.separator())

        addMenuItem(menu, "Toggle Terminal", #selector(MenuTarget.toggleTerminal(_:)), key: "t", modifiers: [.command, .option])
        menu.addItem(.separator())

        addMenuItem(menu, "Toggle Notes", #selector(MenuTarget.toggleNotes(_:)), key: "n", modifiers: .command)
        addMenuItem(menu, "Toggle Health Checks", #selector(MenuTarget.toggleHealthChecks(_:)), key: "h", modifiers: .command)
        menu.addItem(.separator())

        addMenuItem(menu, "Enable Jira", #selector(MenuTarget.toggleJiraPoll(_:)), key: "")
        addMenuItem(menu, "Toggle Jira Window", #selector(MenuTarget.toggleJira(_:)), key: "j", modifiers: .command)
        addMenuItem(menu, "Open Jira Config Window", #selector(MenuTarget.openJiraDashboard(_:)), key: "")
        menu.addItem(.separator())

        addMenuItem(menu, "Confluence Search", #selector(MenuTarget.openConfluence(_:)), key: "")
        addMenuItem(menu, "Confluence Setup…", #selector(MenuTarget.openConfluenceSetup(_:)), key: "")
        addMenuItem(menu, "AI (Grammar Check…)", #selector(MenuTarget.openAI(_:)), key: "")
        addMenuItem(menu, "Compare…", #selector(MenuTarget.openCompare(_:)), key: "")
        menu.addItem(.separator())

        let settingsMenu = NSMenu(title: "Settings")
        let settingsItem = NSMenuItem(title: "Settings", action: nil, keyEquivalent: "")
        settingsItem.submenu = settingsMenu
        menu.addItem(settingsItem)

        if c.commands.contains(where: { $0.kind == .note }) {
            addMenuItem(settingsMenu, "Vim Mode (Notes)", #selector(MenuTarget.toggleVimMode(_:)), key: "")
            for (title, build) in [
                ("Font", { [weak c] (m: NSMenu) in c?.buildFontMenu(into: m) }),
                ("Notes", { [weak c] (m: NSMenu) in c?.buildNotesSettingsMenu(into: m) }),
            ] as [(String, (NSMenu) -> Void)] {
                let sub = NSMenu(title: title)
                let d = DynamicMenuDelegate(build)
                dynamicMenuDelegates.append(d)
                sub.delegate = d
                let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
                item.submenu = sub
                settingsMenu.addItem(item)
            }
        }
        settingsMenu.addItem(.separator())
        addMenuItem(settingsMenu, "Reset Settings to Defaults", #selector(MenuTarget.resetSettings(_:)), key: "")

        item.menu = menu
    }

    private func addMenuItem(_ menu: NSMenu, _ title: String, _ action: Selector, key: String = "", modifiers: NSEvent.ModifierFlags = []) {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = MenuTarget.shared
        item.keyEquivalentModifierMask = modifiers
        menu.addItem(item)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationWillTerminate(_ notification: Notification) {
        wsLog("app terminating (front=\(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"))")
    }

    private func installTextKeys() {
        textKeyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
            TextEditKeys.route(e) ? nil : e
        }
    }

    private func installMainMenu() {
        let mainMenu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "About kitchen-sink", action: nil, keyEquivalent: "")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "Hide", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
        appMenu.addItem(withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        mainMenu.addItem(appItem)

        let fileItem = NSMenuItem()
        let fileMenu = NSMenu(title: "File")
        fileMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        fileMenu.addItem(NSMenuItem.separator())
        let resetItem = NSMenuItem(title: "Reset Window Size", action: #selector(MenuTarget.resetWindowSize(_:)), keyEquivalent: "0")
        resetItem.target = MenuTarget.shared
        resetItem.toolTip = "Reset all windows to their default size"
        fileMenu.addItem(resetItem)
        fileItem.submenu = fileMenu
        mainMenu.addItem(fileItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: #selector(UndoManager.undo), keyEquivalent: "z")
        editMenu.addItem(withTitle: "Redo", action: #selector(UndoManager.redo), keyEquivalent: "Z")
        editMenu.addItem(NSMenuItem.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        mainMenu.addItem(editItem)

        let windowItem = NSMenuItem()
        let windowMenu = NSMenu(title: "Window")
        let resetWinItem = NSMenuItem(title: "Reset Window Size", action: #selector(MenuTarget.resetWindowSize(_:)), keyEquivalent: "0")
        resetWinItem.target = MenuTarget.shared
        windowMenu.addItem(resetWinItem)
        windowMenu.addItem(NSMenuItem.separator())
        windowMenu.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
        windowItem.submenu = windowMenu
        mainMenu.addItem(windowItem)

        NSApp.mainMenu = mainMenu
    }
}

final class MenuTarget: NSObject, NSMenuDelegate {
    static let shared = MenuTarget()
    static weak var controller: SwitcherController?

    static let configIssuesTag = 7401
    static let globalGroupTag = 7402

    func menuNeedsUpdate(_ menu: NSMenu) {
        if let item = menu.item(withTag: MenuTarget.configIssuesTag) {
            _ = readConfigText()
            item.isHidden = configIssues.isEmpty
            item.title = configUsingBackup
                ? "⚠ Config Invalid — Using Backup…"
                : "⚠ Config Warnings (\(configIssues.count))…"
        }
        guard let controller = MenuTarget.controller else { return }
        if let end = menu.items.firstIndex(where: { $0.tag == MenuTarget.globalGroupTag }) {
            var start = end
            while start > 0, menu.items[start - 1].tag == MenuTarget.globalGroupTag + 1 { start -= 1 }
            for _ in start..<end { menu.removeItem(at: start) }
            let group = NSMenu()
            controller.addGlobalWindowItems(to: group)
            for (k, it) in group.items.enumerated() {
                group.removeItem(it)
                it.tag = MenuTarget.globalGroupTag + 1
                menu.insertItem(it, at: start + k)
            }
        }
        let keyWindow = NSApp.keyWindow
        var keyPopup: PopupWindow?
        if let pw = keyWindow?.delegate as? PopupWindow {
            keyPopup = pw
        } else {
            keyPopup = controller.subWindows.first(where: { $0.isShown })
        }

        for item in menu.items {
            switch item.action {
            case #selector(toggleTerminal(_:)):
                item.state = (keyPopup?.terminalShown ?? false) ? .on : .off
            case #selector(toggleNotes(_:)):
                item.state = windowState(for: "notes", controller: controller)
            case #selector(toggleJira(_:)):
                item.state = windowState(for: "jira", controller: controller)
                item.isHidden = !jiraEnabledInConfig()
            case #selector(toggleJiraPoll(_:)):
                item.title = jiraEnabledInConfig() ? "Disable Jira" : "Enable Jira"
            case #selector(openConfluence(_:)), #selector(openConfluenceSetup(_:)):
                item.isHidden = !confluenceEnabled()
            case #selector(openAI(_:)):
                item.isHidden = !aiEnabled()
            case #selector(openCompare(_:)):
                item.isHidden = !compareEnabled()
            case #selector(toggleHealthChecks(_:)):
                item.state = windowState(for: "health-checks", controller: controller)
            case #selector(toggleVimMode(_:)):
                item.state = vimModeEnabled ? .on : .off
            default:
                break
            }
        }
        for item in menu.items {
            if let submenu = item.submenu {
                for subItem in submenu.items {
                    switch subItem.action {
                    case #selector(toggleVimMode(_:)):
                        subItem.state = vimModeEnabled ? .on : .off
                    default:
                        break
                    }
                }
            }
        }
    }

    private func windowState(for name: String, controller: SwitcherController) -> NSControl.StateValue {
        if let w = controller.subWindows.first(where: { $0.config.name == name }) {
            return w.isShown ? .on : .off
        }
        return .off
    }

    @objc func resetWindowSize(_ sender: Any?) {
        NSApp.windows.forEach { w in
            if let pw = w.delegate as? PopupWindow {
                pw.resetToDefaultSize()
            }
        }
    }

    @objc func resetWindowColors(_ sender: Any?) {
        NSApp.windows.forEach { w in
            if let pw = w.delegate as? PopupWindow {
                pw.resetToDefaultColors()
            }
        }
    }

    @objc func closeWindow(_ sender: Any?) {
        if let keyWindow = NSApp.keyWindow {
            keyWindow.performClose(sender)
        }
    }

    @objc func quitApp(_ sender: Any?) {
        NSApp.terminate(sender)
    }

    @objc func toggleTerminal(_ sender: Any?) {
        if let pw = keyPopupWindow() {
            pw.toggleTerminalDrawer()
            pw.setHeaderButtonOn(10, pw.terminalShown)
        }
    }

    @objc func toggleNotes(_ sender: Any?) {
        MenuTarget.controller?.toggleNotes()
    }

    @objc func toggleJira(_ sender: Any?) {
        MenuTarget.controller?.toggleCommand("jira")
    }

    @objc func toggleJiraPoll(_ sender: Any?) {
        MenuTarget.controller?.toggleJiraPoll()
    }

    @objc func openJiraDashboard(_ sender: Any?) {
        MenuTarget.controller?.showJiraDashboard()
    }

    @objc func openSetup(_ sender: Any?) {
        SetupWindow.show(controller: MenuTarget.controller)
    }

    @objc func openConfluence(_ sender: Any?) {
        MenuTarget.controller?.showConfluence()
    }

    @objc func openConfluenceSetup(_ sender: Any?) {
        MenuTarget.controller?.showConfluence(setup: true)
    }

    @objc func openAI(_ sender: Any?) {
        MenuTarget.controller?.showAI()
    }

    @objc func openCompare(_ sender: Any?) {
        MenuTarget.controller?.showCompare()
    }

    @objc func toggleHealthChecks(_ sender: Any?) {
        MenuTarget.controller?.toggleCommand("health-checks")
    }

    private func keyPopupWindow() -> PopupWindow? {
        if let pw = NSApp.keyWindow?.delegate as? PopupWindow {
            return pw
        }
        return MenuTarget.controller?.subWindows.first(where: { $0.isShown })
    }

    private var vimModeEnabled: Bool {
        guard let controller = MenuTarget.controller else { return false }
        return controller.commands.first(where: { $0.name == "notes" })?.vimMode ?? false
    }

    @objc func showConfigIssues(_ sender: Any?) {
        _ = readConfigText()
        let alert = NSAlert()
        alert.alertStyle = configUsingBackup ? .critical : .warning
        alert.messageText = configUsingBackup
            ? "commands.toml is invalid — the last good backup is in use"
            : "commands.toml has \(configIssues.count) warning\(configIssues.count == 1 ? "" : "s")"
        let lines = configIssues.prefix(15).map { i in
            (i.line > 0 ? "line \(i.line): " : "") + i.message
        }
        let more = configIssues.count > 15 ? "\n…and \(configIssues.count - 15) more" : ""
        alert.informativeText = lines.joined(separator: "\n") + more
            + (configUsingBackup
               ? "\n\nBackup: \(configBackupPath)\nRestoring it moves the invalid file aside as commands.toml.broken-<time>."
               : "\n\nInvalid values are ignored; the built-in defaults apply.")
        alert.addButton(withTitle: "Open Config")
        if configUsingBackup { alert.addButton(withTitle: "Restore Backup") }
        alert.addButton(withTitle: "Close")
        NSApp.activate(ignoringOtherApps: true)
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            MenuTarget.controller?.openNoteFile(settings.commandsConfPath)
        case .alertSecondButtonReturn where configUsingBackup:
            _ = restoreConfigFromBackup()
            _ = readConfigText()
        default:
            break
        }
    }

    @objc func toggleVimMode(_ sender: Any?) {
        MenuTarget.controller?.toggleVimModeForNotes()
    }

    @objc func resetSettings(_ sender: Any?) {
        settings.hideOnFocusLoss = true
        removeConfigValue(section: "app", key: "hide-on-focus-loss")
        removeConfigValue(section: "app", key: "float")
        HeaderStyle.current = .flat
        removeConfigValue(section: "app", key: "header-style")
        removeConfigValue(section: "notes", key: "vim-mode")
        removeConfigValue(section: "notes", key: "vim-bin")
    }
}

final class DynamicMenuDelegate: NSObject, NSMenuDelegate {
    private let build: (NSMenu) -> Void
    init(_ build: @escaping (NSMenu) -> Void) { self.build = build }
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        build(menu)
    }
}
var dynamicMenuDelegates: [DynamicMenuDelegate] = []

private var fontFamilyCache: [String: [String]]?
private var fontInstallsRunning: Set<String> = []

enum FontTarget { case editor, terminal }

extension SwitcherController {
    var noteCommandIndex: Int? { commands.firstIndex(where: { $0.kind == .note }) }

    var noteWindow: PopupWindow? {
        guard let i = noteCommandIndex else { return nil }
        return subWindows.first(where: { $0.config.name == commands[i].windowName })
    }

    func addWindowSettingsItems(to menu: NSMenu, window w: PopupWindow, section: String) {
        addThemeMenus(to: menu, window: w, section: section)
        menu.addItem(.separator())
        menu.addItem(menuItem("Reset Default Size") { w.resetToDefaultSize() })
        menu.addItem(menuItem("Reset Default Colors") { [weak self] in
            self?.resetWindowTheme(w, section: section)
        })
    }

    func openConfigMenuItem(_ open: @escaping (String) -> Void) -> NSMenuItem {
        menuItem("Open Config") {
            let fm = FileManager.default
            let canonical = homeDir + "/" + commandsConfName
            let p = fm.fileExists(atPath: canonical) ? canonical : settings.commandsConfPath
            if fm.fileExists(atPath: p) { open(p) }
        }
    }

    private func menuHeader(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    static let fontTypeOrder: [(key: String, label: String)] = [
        ("nerd", "Nerd Fonts"), ("mono", "Monospace"), ("sans", "Sans Serif"),
        ("serif", "Serif"), ("display", "Display & Script"),
    ]

    private static func fontType(_ family: String) -> String? {
        if family.hasPrefix(".") { return nil }
        let lower = family.lowercased()
        if ["emoji", "symbol", "dingbat", "webdings", "wingdings", "lastresort"]
            .contains(where: { lower.contains($0) }) { return nil }
        if family.contains("Nerd Font") { return "nerd" }
        guard let f = NSFontManager.shared.font(withFamily: family, traits: [],
                                                weight: 5, size: 13) else { return nil }
        let traits = f.fontDescriptor.symbolicTraits
        if traits.contains(.monoSpace) || f.isFixedPitch { return "mono" }
        switch (traits.rawValue >> 28) & 0xF {
        case 1...7: return "serif"
        case 8: return "sans"
        case 9, 10: return "display"
        case 12: return nil
        default:
            return lower.contains("serif") && !lower.contains("sans") ? "serif" : "sans"
        }
    }

    static func installedFontsByType() -> [String: [String]] {
        if let c = fontFamilyCache { return c }
        var out: [String: [String]] = [:]
        for fam in NSFontManager.shared.availableFontFamilies.sorted() {
            if let t = fontType(fam) { out[t, default: []].append(fam) }
        }
        fontFamilyCache = out
        return out
    }

    func currentFont(_ target: FontTarget) -> String {
        switch target {
        case .editor: return noteCommandIndex.flatMap { commands[$0].font } ?? "SF Mono"
        case .terminal: return settings.terminalFont
        }
    }

    func currentFontSize(_ target: FontTarget) -> CGFloat {
        switch target {
        case .editor:
            let s = noteCommandIndex.map { commands[$0].fontSize } ?? 0
            return s > 0 ? s : 13
        case .terminal: return settings.terminalFontSize
        }
    }

    func applyFont(_ family: String, target: FontTarget) {
        switch target {
        case .editor:
            guard let i = noteCommandIndex else { return }
            commands[i].font = family
            saveConfigValue(section: commands[i].name, key: "font", value: family)
            noteWindow?.applyFonts(editor: family)
        case .terminal:
            settings.terminalFont = family
            saveConfigValue(section: "app", key: "terminal-font", value: family)
            subWindows.forEach { $0.applyFonts(terminal: family) }
        }
        log("font: \(target == .editor ? "editor" : "terminal") -> \(family)")
    }

    func applyFontSize(_ size: CGFloat, target: FontTarget) {
        let s = min(40, max(8, size))
        let v = s == s.rounded() ? String(Int(s)) : String(format: "%.1f", s)
        switch target {
        case .editor:
            guard let i = noteCommandIndex else { return }
            commands[i].fontSize = s
            saveConfigValue(section: commands[i].name, key: "font-size", value: v)
            noteWindow?.applyFonts(editorSize: s)
        case .terminal:
            settings.terminalFontSize = s
            saveConfigValue(section: "app", key: "terminal-font-size", value: v)
            subWindows.forEach { $0.applyFonts(terminalSize: s) }
        }
        log("font size: \(target == .editor ? "editor" : "terminal") -> \(v)")
    }

    func stepFontSizes(_ delta: Int) {
        applyFontSize(currentFontSize(.editor) + CGFloat(delta), target: .editor)
        applyFontSize(currentFontSize(.terminal) + CGFloat(delta), target: .terminal)
    }

    func buildFontMenu(into menu: NSMenu) {
        let byType = SwitcherController.installedFontsByType()
        for (target, title) in [(FontTarget.editor, "Editor Font"),
                                (FontTarget.terminal, "Terminal Font")] {
            let current = currentFont(target)
            let sub = NSMenu(title: title)
            let types = target == .terminal
                ? SwitcherController.fontTypeOrder.filter { ["nerd", "mono"].contains($0.key) }
                : SwitcherController.fontTypeOrder
            for (key, label) in types {
                guard let fams = byType[key], !fams.isEmpty else { continue }
                let typeMenu = NSMenu(title: label)
                for fam in fams {
                    let item = menuItem(fam, state: fam == current) { [weak self] in
                        self?.applyFont(fam, target: target)
                    }
                    if let f = NSFont(name: fam, size: 13)
                        ?? NSFontManager.shared.font(withFamily: fam, traits: [],
                                                     weight: 5, size: 13) {
                        item.attributedTitle = NSAttributedString(string: fam,
                                                                  attributes: [.font: f])
                    }
                    typeMenu.addItem(item)
                }
                let typeItem = NSMenuItem(title: "\(label) (\(fams.count))", action: nil,
                                          keyEquivalent: "")
                typeItem.submenu = typeMenu
                if fams.contains(current) { typeItem.state = .on }
                sub.addItem(typeItem)
            }
            sub.addItem(.separator())
            let sizeMenu = NSMenu(title: "Size")
            let curSize = currentFontSize(target)
            for n in [11, 12, 13, 14, 15, 16, 18, 20, 24] {
                sizeMenu.addItem(menuItem("\(n) pt", state: CGFloat(n) == curSize) { [weak self] in
                    self?.applyFontSize(CGFloat(n), target: target)
                })
            }
            let sizeItem = NSMenuItem(title: "Size (\(Int(curSize)) pt)", action: nil,
                                      keyEquivalent: "")
            sizeItem.submenu = sizeMenu
            sub.addItem(sizeItem)
            let item = NSMenuItem(title: "\(title): \(current)", action: nil, keyEquivalent: "")
            item.submenu = sub
            menu.addItem(item)
        }
        menu.addItem(.separator())
        let install = NSMenu(title: "Install Font")
        for (key, label) in SwitcherController.fontTypeOrder {
            let entries = settings.fontInstallCasks.filter { $0.type == key }
            guard !entries.isEmpty else { continue }
            install.addItem(menuHeader(label))
            for e in entries {
                let running = fontInstallsRunning.contains(e.cask)
                let installed = SwitcherController.isFontInstalled(e.label)
                let title = running ? "\(e.label) — installing…"
                    : installed ? "\(e.label) — installed" : e.label
                install.addItem(menuItem(title, state: installed ? true : nil,
                                         enabled: !running && !installed) { [weak self] in
                    self?.installFontCask(e.label, cask: e.cask)
                })
            }
        }
        let installItem = NSMenuItem(title: "Install Font…", action: nil, keyEquivalent: "")
        installItem.submenu = install
        menu.addItem(installItem)
        menu.addItem(menuItem("Other Font… (Font Panel)") { [weak self] in
            self?.showSystemFontPanel()
        })
    }

    static func isFontInstalled(_ label: String) -> Bool {
        let norm = { (s: String) in s.lowercased().filter { $0.isLetter || $0.isNumber } }
        let want = norm(label)
        return NSFontManager.shared.availableFontFamilies.contains { norm($0) == want }
    }

    func installFontCask(_ label: String, cask: String) {
        guard !fontInstallsRunning.contains(cask) else { return }
        guard let brew = resolveBinary("brew")
                ?? ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
                    .first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            log("font install: brew not found")
            noteWindow?.setStatus("Install failed: Homebrew (brew) not found", isError: true)
            return
        }
        fontInstallsRunning.insert(cask)
        let before = Set(NSFontManager.shared.availableFontFamilies)
        log("font install: brew install --cask \(cask)")
        noteWindow?.setStatus("Installing \(label)…", isError: false)
        var env = ProcessInfo.processInfo.environment
        env["HOMEBREW_NO_AUTO_UPDATE"] = "1"
        env["HOMEBREW_NO_INSTALL_CLEANUP"] = "1"
        DispatchQueue.global(qos: .userInitiated).async {
            let result = Result { try runProcess(brew, ["install", "--cask", cask], env: env) }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                fontInstallsRunning.remove(cask)
                switch result {
                case .failure(let error):
                    self.log("font install \(cask): \(error)")
                    self.noteWindow?.setStatus("Install failed: \(error.localizedDescription)", isError: true)
                case .success(let r):
                    fontFamilyCache = nil
                    guard r.code == 0 else {
                        let msg = r.err.split(separator: "\n").last.map(String.init) ?? "exit \(r.code)"
                        self.log("font install \(cask) failed: \(r.err)")
                        self.noteWindow?.setStatus("Install failed: \(msg)", isError: true)
                        return
                    }
                    self.log("font install \(cask): ok")
                    self.noteWindow?.setStatus(nil, isError: false)
                    self.offerNewFont(label: label, before: before, tries: 0)
                }
            }
        }
    }

    private func offerNewFont(label: String, before: Set<String>, tries: Int) {
        let now = Set(NSFontManager.shared.availableFontFamilies)
        let added = now.subtracting(before).sorted()
        if added.isEmpty && tries < 10 {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.offerNewFont(label: label, before: before, tries: tries + 1)
            }
            return
        }
        let norm = { (s: String) in s.lowercased().filter { $0.isLetter || $0.isNumber } }
        let family = added.first(where: { norm($0) == norm(label) })
            ?? added.first(where: { !$0.contains("Propo") && !$0.hasSuffix("Mono") })
            ?? added.first
        guard let family else {
            log("font install: \(label) installed, family not registered yet")
            return
        }
        let alert = NSAlert()
        alert.messageText = "Installed \(family)"
        alert.informativeText = "Use it now?"
        alert.addButton(withTitle: "Editor")
        alert.addButton(withTitle: "Terminal")
        alert.addButton(withTitle: "Not Now")
        let apply: (NSApplication.ModalResponse) -> Void = { [weak self] r in
            if r == .alertFirstButtonReturn { self?.applyFont(family, target: .editor) }
            if r == .alertSecondButtonReturn { self?.applyFont(family, target: .terminal) }
        }
        if let w = noteWindow, w.isShown {
            alert.beginSheetModal(for: w.nativeWindow, completionHandler: apply)
        } else {
            NSApp.activate(ignoringOtherApps: true)
            apply(alert.runModal())
        }
    }

    func showSystemFontPanel() {
        let fm = NSFontManager.shared
        fm.target = FontPanelReceiver.shared
        fm.action = #selector(FontPanelReceiver.changeFont(_:))
        FontPanelReceiver.shared.onPick = { [weak self] family in
            self?.applyFont(family, target: .editor)
        }
        let cur = NSFont(name: currentFont(.editor), size: currentFontSize(.editor))
            ?? NSFont.systemFont(ofSize: 13)
        fm.setSelectedFont(cur, isMultiple: false)
        NSApp.activate(ignoringOtherApps: true)
        fm.orderFrontFontPanel(nil)
    }

    func buildNotesSettingsMenu(into menu: NSMenu) {
        guard let i = noteCommandIndex else { return }
        let cmd = commands[i]
        menu.addItem(menuItem("Vim Mode", state: cmd.vimMode) { [weak self] in
            self?.toggleVimModeForNotes()
        })
        let vimMenu = NSMenu(title: "Vim Binary")
        let curBin = (cmd.vimBin as NSString).lastPathComponent
        for name in ["nvim", "vim"] {
            guard let path = resolveBinary(name)
                    ?? ["/opt/homebrew/bin/", "/usr/local/bin/", "/usr/bin/"]
                        .map({ $0 + name })
                        .first(where: { FileManager.default.isExecutableFile(atPath: $0) })
            else { continue }
            let note = name == "vim" ? "  (no tab RPC — keystroke fallback)" : ""
            vimMenu.addItem(menuItem(name + note, state: curBin == name) { [weak self] in
                self?.updateNoteSetting("vim-bin", path, rebuild: cmd.vimMode) { $0.vimBin = path }
            })
        }
        let vimItem = NSMenuItem(title: "Vim Binary: \(curBin)", action: nil, keyEquivalent: "")
        vimItem.submenu = vimMenu
        menu.addItem(vimItem)
        let startMenu = NSMenu(title: "Start With")
        for (value, label) in [("browser", "File Browser"), ("terminal", "Terminal"),
                               ("none", "Nothing (editor only)")] {
            startMenu.addItem(menuItem(label, state: cmd.startDrawer == value) { [weak self] in
                self?.updateNoteSetting("start-drawer", value, rebuild: true) { $0.startDrawer = value }
            })
        }
        let startItem = NSMenuItem(title: "Start With", action: nil, keyEquivalent: "")
        startItem.submenu = startMenu
        menu.addItem(startItem)
        menu.addItem(menuItem("Voice Recording Bar", state: cmd.voice) { [weak self] in
            let v = !cmd.voice
            self?.updateNoteSetting("voice", v ? "true" : "false", rebuild: true) { $0.voice = v }
        })
        menu.addItem(menuItem("Voice: Type As You Speak", state: cmd.voiceLive) { [weak self] in
            let v = !cmd.voiceLive
            self?.updateNoteSetting("voice-live", v ? "true" : "false", rebuild: true) { $0.voiceLive = v }
        })
        menu.addItem(menuItem("Keep Visible When Unfocused (Sticky)", state: cmd.sticky) { [weak self] in
            let v = !cmd.sticky
            self?.updateNoteSetting("sticky", v ? "true" : "false", rebuild: false) { $0.sticky = v }
            self?.noteWindow?.config.sticky = v
        })
        menu.addItem(.separator())
        if let dir = cmd.notesFolder {
            menu.addItem(menuItem("Open Notes Folder in Finder") {
                NSWorkspace.shared.open(URL(fileURLWithPath: dir))
            })
        }
        menu.addItem(menuItem("Reload Config") { [weak self] in
            self?.reloadConfig()
        })
    }

    func updateNoteSetting(_ key: String, _ value: String, rebuild: Bool,
                           _ mutate: (inout CommandSpec) -> Void) {
        guard let i = noteCommandIndex else { return }
        mutate(&commands[i])
        saveConfigValue(section: commands[i].name, key: key, value: value)
        log("notes setting: \(key) = \(value)")
        if rebuild { rebuildNoteWindow() }
    }

    func rebuildNoteWindow() {
        guard let i = noteCommandIndex else { return }
        guard let w = noteWindow else { return }
        let wasShown = w.isShown
        let restoreWID = savedWID
        let restorePID = savedPID
        if wasShown { w.hide(restore: false) } else { w.onEditorClose?(w.currentEditorText) }
        w.shutdownVim()
        subWindows.removeAll { $0 === w }
        w.releaseHooks()
        w.nativeWindow.orderOut(nil)
        if wasShown {
            if settings.sharedWindow {
                slot.memberGone(.notes)
                if ensureSlotMember(.notes, frame: w.baseFrame) { slot.present(.notes) }
                return
            }
            openNoteWindow(commands[i], restoreWID: restoreWID, restorePID: restorePID)
        }
    }

    func restartDaemon() {
        let bundle = Bundle.main.bundlePath
        log("restart requested: relaunching \(bundle)")
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "while kill -0 \(getpid()) 2>/dev/null; do sleep 0.1; done; /usr/bin/open -n -g \"$0\"", bundle]
        try? p.run()
        NSApp.terminate(nil)
    }

    func reloadConfig() {
        commands = loadCommands()
        configureRecentFiles()
        fontFamilyCache = nil
        JiraWords.invalidate()
        log("config reloaded (\(commands.count) commands)")
        rebuildNoteWindow()
        prewarmSlot()
    }
}

private func noteDir(_ p: String) -> String { (p as NSString).deletingLastPathComponent }

private func ensureDefaultNote(in dir: String) -> String {
    let fallback = dir + "/default.md"
    try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    if !FileManager.default.fileExists(atPath: fallback) {
        FileManager.default.createFile(atPath: fallback, contents: nil)
    }
    return fallback
}

extension SwitcherController {
    final class NoteSession {
        unowned let host: SwitcherController
        let cmd: CommandSpec
        let w: PopupWindow
        let vimSocket: String
        var paths: [String]
        var titles: [String]
        var currentPath: String
        var lastSynced: String
        var lastMtime: Date?

        init(host: SwitcherController, cmd: CommandSpec, window: PopupWindow,
             paths: [String], vimSocket: String) {
            self.host = host
            self.cmd = cmd
            self.w = window
            self.vimSocket = vimSocket
            self.paths = paths
            titles = paths.map { URL(fileURLWithPath: $0).lastPathComponent }
            currentPath = paths[0]
            lastSynced = (try? String(contentsOfFile: currentPath, encoding: .utf8)) ?? ""
            lastMtime = mtime(of: currentPath)
        }

        func install() {

            w.imageBaseDir = noteDir(currentPath)
            if noteIsPreview(currentPath) {
                w.setEditorFilePreview(currentPath)
            } else {
                w.editorReadOnly = false
                w.setEditorMarkdown(lastSynced, baseDir: noteDir(currentPath))
            }
            if cmd.vimMode { installVimPane() }
            w.onFontSizeStep = { [host] delta in host.stepFontSizes(delta) }
            w.imageSaver = { [self] img in saveImage(img) }

            w.headerIcon = cmd.icon ?? notesAppIcon
            w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
            w.copyPathButtonLabel = ""
            w.copyConfigButtonLabel = ""
            w.tabTitles = titles
            w.tabPathTip = { [weak self] i in
                guard let self, self.paths.indices.contains(i) else { return nil }
                return (self.paths[i] as NSString).abbreviatingWithTildeInPath
            }
            w.tabFooterText = ""
            w.chromeHeaderTitle = cmd.chromeTitle
            w.onTabChange = { [self] index in loadTab(index) }
            w.onCloseTab = { [self] index in closeNote(index) }
            w.onChromeIconClick = { [self] in showIconMenu() }
            w.onShowShortcuts = { [host, weak w] in
                guard let w else { return }
                host.showShortcuts(on: w, view: "notes")
            }
            w.onAddTab = { [self] in addTab() }
            w.onNewNote = { [self] template in newNote(template: template) }
            if let f = cmd.proseFont { w.proseFont = f }
            if cmd.proseFontSize > 0 { w.proseFontSize = cmd.proseFontSize }
            if cmd.proseWidth > 0 { w.proseWidth = cmd.proseWidth }
            w.proseProvider = { [weak self] in
                guard let self, !noteIsPreview(self.currentPath),
                      let text = try? String(contentsOfFile: self.currentPath, encoding: .utf8) else { return nil }
                return ProseSource(markdown: text, path: self.currentPath)
            }
            w.onSidebarWidthChange = { [cmd] width in
                saveConfigValue(section: cmd.name, key: "sidebar-width", value: String(Int(width)))
            }
            w.onTabClick = { [self] index in
                guard index == w.selectedTab, index < paths.count else { return }
                host.copy(paths[index], "note path: \(paths[index])")
            }
            w.onTabCopyPath = { [self] index in
                guard index < paths.count else { return }
                host.copy(paths[index], "note path: \(paths[index])")
            }
            w.onCopyFilePath = { [self] in
                host.copy(currentPath, "note path: \(currentPath)")
            }
            w.onOpenExternalPath = { [self] path in openExternal(path) }
            w.openNotePaths = { [self] in paths }
            w.onOpenPathPrompt = { [self] in promptOpenPath() }
            w.onTerminalOpenInNotes = { [host] path in
                host.openNoteFile(path)
            }
            w.onTerminalOpenDefault = { path in
                guard FileManager.default.fileExists(atPath: path) else { return }
                NSWorkspace.shared.open(URL(fileURLWithPath: path))
            }
            w.onTerminalRevealInFinder = { path in
                guard FileManager.default.fileExists(atPath: path) else { return }
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            }
            w.onChromeHeaderClick = { [self] in
                host.copy(currentPath, "note path: \(currentPath)")
            }
            if cmd.voice { installVoice() }
            w.onEditorCommit = { [self] text in commitSave(text) }
            w.onEditorClose = { [self] text in commitSave(text) }
            w.onHide = { [self] restore in
                host.dismissPickerIfOpen(for: w)
                host.restoreFocus(restore)
            }
            let t = Timer(timeInterval: noteWatchInterval, repeats: true) { [weak self] _ in
                guard let self, self.w.isShown else { return }
                self.watchTick()
            }
            RunLoop.main.add(t, forMode: .common)
        }

        private func installVimPane() {
            w.setVimPaneActive(!noteIsPreview(currentPath))
            w.vimLaunchArgs = { [self] in
                host.vimArgs(for: cmd, socket: vimSocket,
                             file: noteIsPreview(currentPath) ? nil : currentPath)
            }
            w.onVimExit = { [self] in
                host.log("note '\(cmd.name)': vim exited — relaunching on \(currentPath)")
            }
            let vm = NSMenu(title: "Vim")
            vm.autoenablesItems = false
            vm.addItem(menuItem("Copy") { [weak w] in w?.vimCopy() })
            vm.addItem(menuItem("Paste") { [weak w] in w?.vimPaste() })
            vm.addItem(.separator())
            vm.addItem(menuItem("Copy File Path") { [self] in
                host.copy(currentPath, "note path: \(currentPath)")
            })
            vm.addItem(menuItem("Open in Default App") { [self] in
                NSWorkspace.shared.open(URL(fileURLWithPath: currentPath))
            })
            vm.addItem(menuItem("Reveal in Finder") { [self] in
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: currentPath)])
            })
            vm.addItem(menuItem("Open file at path…") { [weak w] in w?.onOpenPathPrompt?() })
            w.vimMenu = vm
        }

        private func saveImage(_ img: NSImage) -> String? {
            let dir = noteDir(currentPath) + "/assets"
            try? FileManager.default.createDirectory(atPath: dir,
                                                     withIntermediateDirectories: true)
            let name = "img-\(Int(Date().timeIntervalSince1970)).png"
            guard let tiff = img.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff),
                  let data = rep.representation(using: .png, properties: [:]) else { return nil }
            do {
                try data.write(to: URL(fileURLWithPath: dir + "/" + name))
                host.log("note '\(cmd.name)': saved pasted image assets/\(name)")
                return "assets/" + name
            } catch {
                host.log("note '\(cmd.name)': image save failed: \(error)")
                return nil
            }
        }

        func loadTab(_ index: Int) {
            guard index < paths.count else { return }
            let outgoing = currentPath
            if cmd.vimMode {
                w.vimFlush()
            } else if FileManager.default.fileExists(atPath: outgoing), !noteIsPreview(outgoing) {
                host.saveNote(w.currentEditorText, to: outgoing, cmd: cmd)
            }
            var target = paths[index]
            if !FileManager.default.fileExists(atPath: target) {
                let fallback = ensureDefaultNote(in: noteDir(target))
                host.log("note '\(cmd.name)': \(target) deleted — tab now default.md")
                paths[index] = fallback
                titles[index] = URL(fileURLWithPath: fallback).lastPathComponent
                w.tabTitles = titles
                host.removeNotePathFromConfig(target, section: cmd.name)
                host.addNotePathToConfig(fallback, section: cmd.name)
                target = fallback
            }
            currentPath = target
            w.imageBaseDir = noteDir(currentPath)
            if cmd.vimMode {
                let preview = noteIsPreview(currentPath)
                if preview { w.setEditorFilePreview(currentPath) } else { w.vimOpen(currentPath) }
                w.setVimPaneActive(!preview)
                lastSynced = ""
            } else if noteIsPreview(currentPath) {
                w.setEditorFilePreview(currentPath)
                lastSynced = ""
            } else {
                w.editorReadOnly = false
                let loaded = (try? String(contentsOfFile: currentPath, encoding: .utf8)) ?? ""
                w.setEditorMarkdown(loaded, baseDir: noteDir(currentPath))
                lastSynced = loaded
            }
            lastMtime = mtime(of: currentPath)
            w.tabFooterText = ""
            w.copyPathButtonLabel = ""
            w.onChromeHeaderClick = { [self] in
                host.copy(currentPath, "note path: \(currentPath)")
            }
        }

        func closeNote(_ index: Int) {
            guard paths.indices.contains(index) else { return }
            let closing = paths[index]
            let wasCurrent = index == w.selectedTab
            if cmd.vimMode {
                w.vimFlush()
            } else if wasCurrent, FileManager.default.fileExists(atPath: closing),
               !noteIsPreview(closing) {
                host.saveNote(w.currentEditorText, to: closing, cmd: cmd)
            }
            host.log("note '\(cmd.name)': closed \(closing)")
            DismissedNotes.add(closing)
            host.removeNotePathFromConfig(closing, section: cmd.name)
            paths.remove(at: index)
            titles.remove(at: index)
            if paths.isEmpty {
                let fallback = ensureDefaultNote(in: noteDir(closing))
                paths = [fallback]
                titles = [URL(fileURLWithPath: fallback).lastPathComponent]
                host.addNotePathToConfig(fallback, section: cmd.name)
            }
            w.tabTitles = titles
            if wasCurrent {
                let next = min(index, paths.count - 1)
                if w.selectedTab == next {
                    loadTab(next)
                } else {
                    w.selectedTab = next
                }
            } else if index < w.selectedTab {
                w.selectedTab -= 1
            }
        }

        private func showIconMenu() {
            let menu = NSMenu()
            menu.autoenablesItems = false
            host.addGlobalWindowItems(to: menu)
            menu.addItem(.separator())

            func toggleItem(_ title: String, _ state: Bool, _ action: @escaping () -> Void) {
                menu.addItem(menuItem(title, state: state, action))
            }

            if cmd.terminal {
                toggleItem("Toggle Terminal", w.terminalShown) { [w] in
                    w.toggleTerminalDrawer()
                }
            }
            if cmd.voice {
                let micShown = w.meterEnabled
                toggleItem(micShown ? "Mute Microphone" : "Enable Microphone", micShown) { [w] in
                    let shown = !w.meterEnabled
                    w.meterEnabled = shown
                }
            }
            if cmd.kind == .note {
                let vimOn = host.noteCommandIndex.map { host.commands[$0].vimMode } ?? cmd.vimMode
                toggleItem("Vim Mode", vimOn) { [host] in
                    host.toggleVimModeForNotes()
                }
                if settings.sharedWindow { menu.addItem(host.escHidesMenuItem(.notes)) }
            }
            menu.addItem(.separator())
            let fontMenu = NSMenu(title: "Font")
            host.buildFontMenu(into: fontMenu)
            let fontItem = NSMenuItem(title: "Font", action: nil, keyEquivalent: "")
            fontItem.submenu = fontMenu
            menu.addItem(fontItem)
            let notesMenu = NSMenu(title: "Notes Settings")
            host.buildNotesSettingsMenu(into: notesMenu)
            let notesItem = NSMenuItem(title: "Notes Settings", action: nil, keyEquivalent: "")
            notesItem.submenu = notesMenu
            menu.addItem(notesItem)
            menu.addItem(.separator())

            host.addWindowSettingsItems(to: menu, window: w, section: cmd.name)
            menu.addItem(.separator())
            menu.addItem(host.openConfigMenuItem { [w] in w.onOpenExternalPath?($0) })
            menu.addItem(host.shortcutsMenuItem(for: w, view: "notes"))

            w.showHeaderMenu(menu)
        }

        private func addTab() {
            let panel = w.nativeWindow
            panel.makeKeyAndOrderFront(nil)
            let chooser = NSAlert()
            chooser.messageText = "Add a note"
            chooser.informativeText = "Open an existing file, or create a new note:"
            chooser.addButton(withTitle: "Open Existing…")
            chooser.addButton(withTitle: "New Note")
            chooser.addButton(withTitle: "Cancel").keyEquivalent = "\u{1b}"
            chooser.beginSheetModal(for: panel) { [self] response in
                switch response {
                case .alertFirstButtonReturn:
                    presentPathSheet(on: panel,
                                     title: "Open note",
                                     message: "Path to open as a note:",
                                     okTitle: "Open") { [self] value in
                        guard let value else { return }
                        let raw = value.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !raw.isEmpty else { return }
                        let expanded = (raw as NSString).expandingTildeInPath
                        let mdPath = (expanded as NSString).deletingPathExtension + ".md"
                        let chosen = FileManager.default.fileExists(atPath: mdPath)
                            ? mdPath : expanded
                        let p = (chosen as NSString).standardizingPath
                        if FileManager.default.fileExists(atPath: p) {
                            w.onOpenExternalPath?(p)
                        } else {
                            host.log("note '\(cmd.name)': no such path \(p)")
                        }
                    }
                case .alertSecondButtonReturn:
                    presentPathSheet(on: panel,
                                     title: "New note",
                                     message: "Name for the new note:",
                                     okTitle: "Create") { [self] value in
                        guard let value else { return }
                        var name = value.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !name.isEmpty else {
                            host.log("note '\(cmd.name)': empty name, not creating")
                            return
                        }
                        if !name.hasSuffix(".md") { name += ".md" }
                        let newPath = noteDir(paths[0]) + "/" + name
                        if let idx = paths.firstIndex(of: newPath) {
                            w.selectedTab = idx
                            return
                        }
                        if !FileManager.default.fileExists(atPath: newPath) {
                            FileManager.default.createFile(atPath: newPath, contents: nil)
                        }
                        appendTab(newPath, logged: "created")
                    }
                default:
                    break
                }
            }
        }

        func newNote(template: Bool) {
            guard let dir = cmd.notesFolder ?? paths.first.map(noteDir) else { return }
            let fm = FileManager.default
            let base = (template ? cmd.newDocName : cmd.newNoteName)
                .components(separatedBy: .whitespaces).filter { !$0.isEmpty }.joined(separator: "-")
            var n = 1, path = ""
            repeat {
                path = dir + "/\(base)-\(n).md"
                n += 1
            } while fm.fileExists(atPath: path) || paths.contains(path)
            let body = snippetText(cmd.newDocTemplate)
            if body == nil {
                host.log("note '\(cmd.name)': no snippet \(cmd.newDocTemplate) in vim/snippets/markdown.json — empty note")
            }
            guard fm.createFile(atPath: path, contents: Data((body ?? "").utf8)) else {
                host.log("note '\(cmd.name)': could not create \(path)")
                return
            }
            appendTab(path, logged: "created from \(cmd.newDocTemplate)")
        }

        private func snippetText(_ name: String) -> String? {
            let file = assetDir + "/vim/snippets/markdown.json"
            guard let data = FileManager.default.contents(atPath: file),
                  let all = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let snip = all[name] as? [String: Any] else { return nil }
            var text = (snip["body"] as? [String])?.joined(separator: "\n") ?? (snip["body"] as? String ?? "")
            for (pattern, with) in [(#"\$\{\d+:([^}]*)\}"#, "$1"), (#"\$\{\d+\}|(?<!\\)\$\d+"#, ""), (#"\\\$"#, "\\$")] {
                text = text.replacingOccurrences(of: pattern, with: with, options: .regularExpression)
            }
            return text.hasSuffix("\n") ? text : text + "\n"
        }

        private func appendTab(_ p: String, logged verb: String) {
            paths.append(p)
            titles.append(URL(fileURLWithPath: p).lastPathComponent)
            w.tabTitles = titles
            w.selectedTab = paths.count - 1
            DismissedNotes.remove(p)
            host.addNotePathToConfig(p, section: cmd.name)
            host.log("note '\(cmd.name)': \(verb) \(p)")
        }

        private func openExternal(_ path: String) {
            let p = (path as NSString).standardizingPath
            if let idx = paths.firstIndex(of: p) {
                w.selectedTab = idx
                return
            }
            guard FileManager.default.fileExists(atPath: p) else {
                host.log("note '\(cmd.name)': cannot open \(p) — missing")
                return
            }
            appendTab(p, logged: "opened")
        }

        private func promptOpenPath() {
            presentPathSheet(on: w.nativeWindow,
                             title: "Open file at path",
                             message: "Absolute path (or ~/…) to open as a note:",
                             okTitle: "Open") { [self] value in
                guard let value else { return }
                let raw = value.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !raw.isEmpty else { return }
                let p = ((raw as NSString).expandingTildeInPath as NSString).standardizingPath
                if FileManager.default.fileExists(atPath: p) {
                    w.onOpenExternalPath?(p)
                } else {
                    host.log("note '\(cmd.name)': no such path \(p)")
                }
            }
        }

        func commitSave(_ text: String) {
            guard !noteIsPreview(currentPath) else { return }
            if cmd.vimMode {
                w.vimFlush()
                lastMtime = mtime(of: currentPath)
                w.tabFooterText = ""
                return
            }
            if FileManager.default.fileExists(atPath: currentPath) {
                host.saveNote(text, to: currentPath, cmd: cmd)
                lastSynced = text
                lastMtime = mtime(of: currentPath)
                w.tabFooterText = ""
                return
            }
            let deadPath = currentPath
            let fallback = ensureDefaultNote(in: noteDir(deadPath))
            host.log("note '\(cmd.name)': \(deadPath) deleted — text parked in \(fallback)")
            host.saveNote(text, to: fallback, cmd: cmd)
            if let idx = paths.firstIndex(of: deadPath) {
                paths[idx] = fallback
                titles[idx] = URL(fileURLWithPath: fallback).lastPathComponent
            } else {
                paths.append(fallback)
                titles.append(URL(fileURLWithPath: fallback).lastPathComponent)
            }
            w.tabTitles = titles
            host.removeNotePathFromConfig(deadPath, section: cmd.name)
            host.addNotePathToConfig(fallback, section: cmd.name)
            currentPath = fallback
            lastSynced = text
            lastMtime = mtime(of: fallback)
            w.tabFooterText = ""
        }

        func saveEditorText() {
            commitSave(w.currentEditorText)
        }

        private func watchTick() {
            var dirty = false
            var i = 0
            while i < paths.count {
                let p = paths[i]
                if FileManager.default.fileExists(atPath: p) { i += 1; continue }
                let fallback = ensureDefaultNote(in: noteDir(p))
                if p == currentPath {
                    let vimText = cmd.vimMode && !noteIsPreview(p)
                        ? w.vimEval("join(getline(1, '$'), \"\\n\")") : nil
                    let text = noteIsPreview(p) ? "" : (vimText ?? w.currentEditorText)
                    host.log("note '\(cmd.name)': \(p) deleted on disk — text parked in \(fallback)")
                    host.saveNote(text, to: fallback, cmd: cmd)
                    currentPath = fallback
                    lastSynced = text
                    lastMtime = mtime(of: fallback)
                    w.tabFooterText = ""
                    w.setEditorMarkdown(text, baseDir: noteDir(fallback))
                    w.imageBaseDir = noteDir(fallback)
                    if cmd.vimMode {
                        w.vimCommand("silent! bwipeout! " + p.replacingOccurrences(of: " ", with: "\\ "))
                        w.vimOpen(fallback)
                        w.setVimPaneActive(true)
                    }
                } else {
                    host.log("note '\(cmd.name)': \(p) deleted on disk — tab now default.md")
                }
                host.removeNotePathFromConfig(p, section: cmd.name)
                if let existing = paths.firstIndex(of: fallback), existing != i {
                    paths.remove(at: i)
                    titles.remove(at: i)
                } else {
                    paths[i] = fallback
                    titles[i] = URL(fileURLWithPath: fallback).lastPathComponent
                    host.addNotePathToConfig(fallback, section: cmd.name)
                    i += 1
                }
                dirty = true
            }
            for p in expandPaths(cmd.paths, extensions: ["md"])
            where FileManager.default.fileExists(atPath: p) && !paths.contains(p)
                && !DismissedNotes.contains(p) {
                paths.append(p)
                titles.append(URL(fileURLWithPath: p).lastPathComponent)
                host.log("note '\(cmd.name)': new note detected — added tab \(p)")
                dirty = true
            }
            if dirty {
                w.tabTitles = titles
                if let active = paths.firstIndex(of: currentPath), active != w.selectedTab {
                    w.selectedTab = active
                }
            }
            if cmd.vimMode, let mt = mtime(of: currentPath), !noteIsPreview(currentPath) {
                if let last = lastMtime, mt != last {
                    w.vimCommand("silent! checktime")
                    w.tabFooterText = ""
                }
                lastMtime = mt
            } else if let mt = mtime(of: currentPath), !noteIsPreview(currentPath) {
                if let last = lastMtime, mt != last {
                    if w.currentEditorText == lastSynced {
                        let newText = (try? String(contentsOfFile: currentPath, encoding: .utf8)) ?? ""
                        if newText != lastSynced {
                            w.setEditorMarkdown(newText, baseDir: noteDir(currentPath))
                            lastSynced = newText
                            w.tabFooterText = ""
                            host.log("note '\(cmd.name)': reloaded \(currentPath) after external write")
                        }
                    } else {
                        host.log("note '\(cmd.name)': external change to \(currentPath) ignored (unsaved edits)")
                    }
                }
                lastMtime = mt
            }
        }

        private func installVoice() {
            let w = self.w, cmd = self.cmd, host = self.host
            host.log("voice '\(cmd.name)': voice controls enabled (\(cmd.voiceLive ? "live" : "insert on stop"))")
            let voice = VoiceRecorder()
            let live = cmd.voiceLive
            var committedStr = ""
            var draft = ""
            var sessionActive = false
            var vimAnchored = false
            var anchor: NSRange?
            var pre = "", post = ""
            var liveWrite: Timer?
            var pendingDraw: DispatchWorkItem?
            var lastDraw = Date.distantPast
            let dbgPath = NSString(string: "~/.cache/ws-voice-debug.log")
                .expandingTildeInPath
            func dbg(_ s: String) { appendToFile(dbgPath, "\(Date()) \(s)\n") }
            func regionText() -> String {
                [committedStr, draft].filter { !$0.isEmpty }.joined(separator: " ")
            }
            func beginRegion() {
                committedStr = ""
                draft = ""
                sessionActive = true
                if cmd.vimMode {
                    vimAnchored = w.vimVoiceBegin()
                    dbg("record start vim anchored=\(vimAnchored) live=\(live)")
                    return
                }
                let sel = w.editorSelection
                let text = w.editorText as NSString
                let loc = min(sel.location + sel.length, text.length)
                func isText(_ i: Int) -> Bool {
                    guard i >= 0, i < text.length else { return false }
                    return !(Character(UnicodeScalar(text.character(at: i)) ?? " ").isWhitespace)
                }
                pre = isText(loc - 1) ? " " : ""
                post = isText(loc) ? " " : ""
                anchor = NSRange(location: loc, length: 0)
                dbg("record start at \(loc) of \(text.length) live=\(live)")
            }
            func drawRegion() {
                pendingDraw?.cancel()
                pendingDraw = nil
                lastDraw = Date()
                let body = regionText()
                if cmd.vimMode {
                    if vimAnchored && !w.vimVoiceUpdate(body) {
                        vimAnchored = false
                        dbg("vim region lost (buffer unloaded?) - falling back to append")
                    }
                    return
                }
                guard var a = anchor else { return }
                let full = body.isEmpty ? "" : pre + body + post
                a.length = w.replaceRange(a, with: full,
                                          caretBack: body.isEmpty ? 0 : (post as NSString).length)
                anchor = a
            }
            func scheduleDraw() {
                guard pendingDraw == nil else { return }
                let item = DispatchWorkItem { drawRegion() }
                pendingDraw = item
                DispatchQueue.main.asyncAfter(
                    deadline: .now() + max(0, 0.2 - Date().timeIntervalSince(lastDraw)), execute: item)
            }
            func vimAppendFallback(_ text: String) {
                guard !text.isEmpty else { return }
                if !w.vimAppend("\n" + text, to: self.currentPath) {
                    let old = (try? String(contentsOfFile: self.currentPath, encoding: .utf8)) ?? ""
                    var new = old
                    if !new.isEmpty && !new.hasSuffix("\n") { new += "\n" }
                    new += "\n" + text + "\n"
                    try? new.write(toFile: self.currentPath, atomically: true, encoding: .utf8)
                    w.vimCommand("silent! checktime")
                }
            }
            func save() {
                if cmd.vimMode { w.vimFlush() } else { self.saveEditorText() }
            }
            func finishSession() {
                guard sessionActive else { return }
                sessionActive = false
                liveWrite?.invalidate()
                liveWrite = nil
                committedStr = regionText()
                draft = ""
                if cmd.vimMode && !vimAnchored {
                    vimAppendFallback(committedStr)
                } else {
                    drawRegion()
                }
                if cmd.vimMode { w.vimVoiceEnd() } else { save() }
                anchor = nil
                dbg("session done +\(committedStr.count) chars")
                if committedStr.isEmpty { host.log("voice '\(cmd.name)': no speech detected") }
                committedStr = ""
                w.tabFooterText = ""
            }
            w.onMeterRecord = {
                switch voice.state {
                case .idle:
                    beginRegion()
                    w.tabFooterText = live ? "🎙 dictating at the cursor" : "🎙 listening — inserted at the cursor on stop"
                    if !cmd.vimMode && live {
                        liveWrite = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { _ in
                            if !regionText().isEmpty { self.saveEditorText() }
                        }
                    }
                    voice.start()
                    if voice.state == .idle { finishSession() }
                case .recording, .paused: voice.stop()
                case .transcribing: break
                }
            }
            w.onMeterPause = {
                if voice.state == .recording {
                    voice.pause()
                } else if voice.state == .paused {
                    voice.resume()
                }
            }
            voice.onStateChange = { [weak w] state in
                guard let w else { return }
                w.recordingState = voice.state.rawValue
                w.recordingElapsed = voice.elapsed
                if state == .idle { finishSession() }
            }
            voice.onLevel = { [weak w] level in
                guard let w else { return }
                w.recordingLevel = level
                w.recordingElapsed = voice.elapsed
            }
            voice.onPartial = { [weak w] text in
                guard let w, !text.isEmpty, sessionActive else { return }
                draft = text
                if live && (!cmd.vimMode || vimAnchored) {
                    scheduleDraw()
                } else {
                    w.tabFooterText = "🎙 " + String(regionText().suffix(80))
                }
            }
            voice.onBatch = { text in
                let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty && sessionActive {
                    committedStr += (committedStr.isEmpty ? "" : " ") + trimmed
                    draft = ""
                    dbg("batch +\(trimmed.count) committed=\(committedStr.count)")
                    if live {
                        if cmd.vimMode && !vimAnchored {
                            vimAppendFallback(trimmed)
                            committedStr = ""
                        } else {
                            drawRegion()
                            save()
                        }
                    }
                } else if !sessionActive {
                    dbg("batch after the session ended dropped (+\(trimmed.count))")
                }
                if voice.state == .transcribing {
                    finishSession()
                    voice.resetSession()
                }
            }
            voice.onError = { [weak w] err in
                host.log("voice '\(cmd.name)': \(err)")
                dbg("error: \(err)")
                voice.resetSession()
                w?.tabFooterText = "⚠️ \(err)"
            }
            w.onHideVoiceStop = {
                liveWrite?.invalidate()
                liveWrite = nil
                voice.stop()
            }
        }
    }
}

extension SwitcherController {
    final class ListSession {
        static func isJira(_ cmd: CommandSpec) -> Bool {
            cmd.name == "jira" || cmd.name == jiraReleasesWindow
        }
        static func inSlot(_ cmd: CommandSpec) -> Bool { settings.sharedWindow && isJira(cmd) }
        static func tabColumns(_ cmd: CommandSpec, _ path: String?) -> [ListColumn] {
            guard cmd.table else { return [] }
            guard isJira(cmd) else { return cmd.columns }
            if let path, let own = JiraPoll.owner(ofTab: path), !own.columns.isEmpty {
                return JiraPoll.labeled(own.columns)
            }
            return JiraPoll.labeled(cmd.columns)
        }
        static let emptyValue = "\u{0}none"
        static let multiValued: Set<String> = ["labels", "release", "releaseLabel", "releaseDate", "components",
                                               "fixVersions"]
        static let personFields: Set<String> = ["assignee", "reporter", "creator"]

        unowned let host: SwitcherController
        let cmd: CommandSpec
        let w: PopupWindow
        let isJira: Bool
        let isReleaseView: Bool
        let configSection: String
        let inSlot: Bool
        let cap: Int
        let copyKeys: [String]
        let fieldLabels: [String: String]
        let restoreWID: String?
        let restorePID: pid_t?

        var tabs: [(path: String, items: [FieldRow])] { didSet { invalidateFilter() } }
        var currentTab = 0 { didSet { invalidateFilter() } }
        var columns: [ListColumn]
        var activeDims: [String] = []
        var colFilters: [String: Set<String>] = [:]
        var peopleCache: [String: (title: String, detail: String)]?
        var visibleOffset = 0
        var reloadWatcher: Timer?
        var sortKey: (field: String, ascending: Bool)?
        var favKeys: Set<String>
        var favReleases: [String] = []
        var pinnedLabels: [String] = []
        static let labelPinPrefix = "label:"
        var pinnedBoards: [String] = []
        var boards: [String: JiraPoll.BoardInfo] = [:]
        var catalog: [JiraPoll.CatalogBoard] = []
        var catalogStamp: Date?
        var pinnedViews: [String] = []
        static let viewPinPrefix = "view:"
        var boardSprint: [String: String] = [:]
        var sprintKeys: Set<String>?
        var sprintKeyCache: [String: (keys: Set<String>, at: Date)] = [:]
        var boardBar: JiraBoardBar?
        var boardCols: JiraBoardColumnsView?
        static let boardPinPrefix = "board:", myWorkPrefix = "mywork:"
        static let myWork = [("mine", "Assigned to me", "person"), ("reported", "Reported by me", "square.and.pencil"),
                             ("today", "Updated today", "clock"), ("watching", "Watching", "eye")]
        var pinView: (key: String, path: String, items: [FieldRow])?
        var pinStamp: Date?
        var groupBy: String?
        var boardGroupBy = "boardColumn"
        var collapsedGroups: Set<String> = []
        static let quickDim = "__quick"
        var quickKeys: Set<String>?
        var statusCats: [String: String]?
        var openPicker: JiraMultiPicker?
        var resizeSave: DispatchWorkItem?
        var tabMtimes: [Date?]
        var badgeStamp: (status: Date?, at: Date) = (nil, .distantPast)
        var cacheStamp: Date?
        var filterIndex: FuzzyIndex?
        var sortRanks: (field: String, ascending: Bool, ranks: [Int])?
        var filterGen = 0
        var filterPrewarmQueued = false

        init(host: SwitcherController, cmd: CommandSpec, window: PopupWindow,
             tabs: [(path: String, items: [FieldRow])], columns: [ListColumn],
             restoreWID: String?, restorePID: pid_t?) {
            self.host = host
            self.cmd = cmd
            self.w = window
            self.tabs = tabs
            self.columns = columns
            self.restoreWID = restoreWID
            self.restorePID = restorePID
            isJira = ListSession.isJira(cmd)
            isReleaseView = cmd.name == jiraReleasesWindow
            configSection = isReleaseView ? "jira" : cmd.name
            inSlot = ListSession.inSlot(cmd)
            cap = cmd.maxRows > 0 ? cmd.maxRows : Int.max
            copyKeys = cmd.copyFields
            fieldLabels = isJira ? JiraPoll.fieldLabels() : [:]
            favKeys = isJira ? JiraPoll.favorites() : []
            favReleases = isJira ? JiraPoll.favoriteReleases() : []
            pinnedLabels = isJira ? JiraPoll.pinnedLabels() : []
            pinnedBoards = isJira ? JiraPoll.pinnedBoards() : []
            pinnedViews = isJira ? JiraPoll.pinnedViews() : []
            catalog = isJira ? JiraPoll.boardCatalog() : []
            catalogStamp = mtime(of: JiraPoll.boardCatalogPath)
            groupBy = isJira ? UserDefaults.standard.string(forKey: "listGroupBy.\(cmd.name)") : nil
            boards = isJira ? JiraPoll.boardsInfo() : [:]
            tabMtimes = tabs.map { mtime(of: $0.path) }
            cacheStamp = mtime(of: JiraPoll.issueCachePath)
        }

        func currentItems() -> [FieldRow] { pinView?.items ?? tabs[currentTab].items }
        var currentPath: String { pinView?.path ?? tabs[currentTab].path }

        func cellValues(_ field: String, _ row: FieldRow) -> [String] {
            let v = (row.fields[field] ?? "").trimmingCharacters(in: .whitespaces)
            guard !v.isEmpty else { return [ListSession.emptyValue] }
            guard ListSession.multiValued.contains(field) else { return [v] }
            let parts = v.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
            return parts.isEmpty ? [ListSession.emptyValue] : parts
        }

        func label(_ field: String) -> String {
            field == ListSession.quickDim ? "Quick filters" : fieldLabels[field] ?? field
        }

        func barDims() -> [String] {
            let colFields = Set(columns.filter(\.filterable).map(\.field))
            return cmd.filters.filter { f in
                !colFields.contains(f)
                    && !(f == "release" && colFields.contains("releaseLabel"))
                    && !(f == "releaseLabel" && colFields.contains("release"))
            }
        }

        func peopleByValue() -> [String: (title: String, detail: String)] {
            if let c = peopleCache { return c }
            var people: [String: (title: String, detail: String)] = [:]
            for u in JiraDirectory.load().users {
                for k in [u.username, u.name, u.id] where !k.isEmpty && people[k] == nil {
                    let handle = [u.username, u.email].filter { !$0.isEmpty && $0 != u.name && $0 != k }
                    people[k] = (k == u.name ? u.name : "\(u.name) (\(k))", handle.joined(separator: " · "))
                }
            }
            peopleCache = people
            return people
        }

        func filterOptions(_ field: String) -> [JiraMultiPicker.Option] {
            let emptyValue = ListSession.emptyValue
            var counts: [String: Int] = [:]
            var order: [String] = []
            for row in currentItems() {
                for v in cellValues(field, row) {
                    if counts[v] == nil { order.append(v) }
                    counts[v, default: 0] += 1
                }
            }
            let people = ListSession.personFields.contains(field) ? peopleByValue() : [:]
            let sorted = order.enumerated().sorted { a, b in
                let ca = counts[a.element] ?? 0, cb = counts[b.element] ?? 0
                if (a.element == emptyValue) != (b.element == emptyValue) { return b.element == emptyValue }
                return ca != cb ? ca > cb : a.element.localizedStandardCompare(b.element) == .orderedAscending
            }.map(\.element)
            return sorted.map { v in
                let n = counts[v] ?? 0
                let count = "\(n) row\(n == 1 ? "" : "s")"
                if v == emptyValue {
                    return .init(id: v, title: field == "assignee" ? "(unassigned)" : "(empty)", detail: count)
                }
                if let p = people[v] {
                    return .init(id: v, title: p.title, detail: ([p.detail, count].filter { !$0.isEmpty })
                                    .joined(separator: " · "))
                }
                return .init(id: v, title: v, detail: count)
            }
        }

        func filterSummary(_ field: String) -> String {
            let picked = colFilters[field] ?? []
            if picked.isEmpty { return "All" }
            if picked.count == 1, let v = picked.first {
                if field == ListSession.quickDim {
                    return onBoard.flatMap { boards[$0]?.quickFilters.first { $0.id == v }?.name } ?? v
                }
                if v == ListSession.emptyValue { return "(empty)" }
                return ListSession.personFields.contains(field) ? peopleByValue()[v]?.title ?? v : v
            }
            return "\(picked.count) selected"
        }

        func updateFilterIndicators() {
            w.tableFilterActive = Set(columns.indices.filter { !(colFilters[columns[$0].field] ?? []).isEmpty })
            w.filterSummaries = activeDims.map(filterSummary)
            w.filterActive = Set(activeDims.indices.filter { !(colFilters[activeDims[$0]] ?? []).isEmpty })
        }

        func applyFilterData() {
            let items = currentItems()
            activeDims = barDims().filter { f in items.contains { !($0.fields[f] ?? "").isEmpty } }
            if let b = onBoard, !(boards[b]?.quickFilters.isEmpty ?? true) { activeDims.append(ListSession.quickDim) }
            if !activeDims.contains(ListSession.quickDim) { quickKeys = nil }
            colFilters = colFilters.filter { f, _ in
                columns.contains { $0.field == f } || activeDims.contains(f) }
            w.filterLabels = activeDims.map(label)
            w.filterValues = activeDims.map { _ in ["All"] }
            w.filterValueLabels = []
            w.filterSelections = Array(repeating: 0, count: activeDims.count)
            updateFilterIndicators()
            w.growWidthToContent()
        }

        func syncSortArrow() {
            w.tableSort = sortKey.flatMap { k in
                columns.firstIndex(where: { $0.field == k.field }).map { ($0, k.ascending) }
            }
        }

        func ensureTab(_ file: String) {
            guard cmd.name == "jira", !tabs.contains(where: { ($0.path as NSString).lastPathComponent == file }),
                  tabs.indices.contains(currentTab) else { return }
            host.pendingJiraTab = (tabs[currentTab].path as NSString).lastPathComponent
            host.reloadJiraWindow()
        }

        func setFavorite(_ rows: [FieldRow], on: Bool) {
            let keys = rows.compactMap { $0.fields["key"] }.filter { !$0.isEmpty }
            guard !keys.isEmpty else { return }
            let before = favKeys
            if on { favKeys.formUnion(keys) } else { favKeys.subtract(keys) }
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            let s = keys.count == 1 ? keys[0] : "\(keys.count) issues"
            w.showToast(on ? "Pinned \(s) to favorites" : "Unpinned \(s)", symbol: on ? "star.fill" : "star")
            let json = (try? JSONSerialization.data(withJSONObject: rows.map { $0.fields.filter { !$0.key.hasPrefix("__") } }))
                .map { String(decoding: $0, as: UTF8.self) } ?? "[]"
            JiraPoll.run("jira_poll.py", ["--favorite", on ? "add" : "remove"] + keys, stdin: json) {
                [self] code, _, err in
                host.log("jira: favorite \(on ? "add" : "remove") \(keys.joined(separator: ",")) (exit \(code))"
                         + (code == 0 ? "" : " " + err))
                guard code == 0 else {
                    favKeys = before
                    w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
                    w.showToast("Favorites not saved: \(JiraPoll.errorLine(err, fallback: "error"))",
                                symbol: "exclamationmark.triangle")
                    return
                }
                ensureTab(JiraPoll.favoritesFile)
            }
        }

        enum Pin { case myWork(String), board(String), view(String), label(String), release(String), boardList }
        func pin(_ k: String) -> Pin {
            if k == ListSession.boardListKey { return .boardList }
            for (pre, f) in [(ListSession.myWorkPrefix, Pin.myWork), (ListSession.boardPinPrefix, Pin.board),
                             (ListSession.viewPinPrefix, Pin.view), (ListSession.labelPinPrefix, Pin.label)] as [(String, (String) -> Pin)]
                where k.hasPrefix(pre) { return f(String(k.dropFirst(pre.count))) }
            return .release(k)
        }
        var showMyWork: Bool { (configSectionValue("jira", "my-work") ?? "true") != "false" }
        func boardName(_ id: String) -> String {
            boards[id]?.name ?? JiraDirectory.load().boards.first { $0.id == id }?.name ?? "Board \(id)"
        }

        static let boardListKey = "boards:all"
        var onBoardList: Bool { pinView?.key == ListSession.boardListKey }
        var sidebarBoards: [String] { pinnedBoards }
        var backToBoardList = false
        static let boardListColumns = ListColumn.parse(
            "name:Board:40::sort+filter,project:Project:12::sort+filter,type:Type:16::sort+filter,"
            + "sprint:Current sprint:24::sort+filter,boardId:ID:8:right:sort")

        func boardListRows() -> [FieldRow] {
            let ids = catalog.map(\.id)
            return (ids + pinnedBoards.filter { !ids.contains($0) }).map { id in
                let c = catalogBoard(id)
                let name = c?.name ?? boardName(id), proj = c?.project ?? ""
                let type = c.map { $0.type == "simple" ? "team-managed" : $0.type } ?? ""
                let sprint = c?.sprints.first { $0.state == "active" }?.name ?? ""
                var r = FieldRow(title: name, content: nil, trailing: nil, detail: nil, body: nil,
                                 searchText: [name, proj, type, sprint, id].joined(separator: " "),
                                 fields: ["__board": id, "boardId": id, "name": name, "project": proj,
                                          "type": type, "sprint": sprint])
                r.starred = pinnedBoards.contains(id)
                return r
            }
        }

        func showBoardList() {
            backToBoardList = false
            pinView = (ListSession.boardListKey, JiraPoll.boardCatalogPath, boardListRows())
            w.selectSidebarPin(ListSession.boardListKey)
            showRows(columns: ListSession.boardListColumns)
            host.log("list '\(cmd.name)': board list (\(pinView?.items.count ?? 0) boards)")
        }

        func refreshBoardList() {
            guard onBoardList else { return }
            pinView?.items = boardListRows()
            invalidateFilter()
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
        }

        func openListedBoard(_ b: String) {
            let fromList = onBoardList
            openBoard(b)
            backToBoardList = fromList
        }

        func syncReleasePins() {
            guard cmd.name == "jira" else { return }
            let ids = (showMyWork ? ListSession.myWork.map { ListSession.myWorkPrefix + $0.0 } : [])
                + pinnedViews.map { ListSession.viewPinPrefix + $0 }
                + favReleases
                + pinnedLabels.map { ListSession.labelPinPrefix + $0 }
                + (catalog.isEmpty && pinnedBoards.isEmpty ? [] : [ListSession.boardListKey])
                + sidebarBoards.map { ListSession.boardPinPrefix + $0 }
            if let k = pinView?.key, !ids.contains(k) { leavePinView() }
            w.setSidebarPinned(ids, title: "Pinned", icon: "pin.fill",
                               selected: sidebarSelection,
                               label: { [self] k in
                                   switch pin(k) {
                                   case .myWork(let v): return ListSession.myWork.first { $0.0 == v }?.1 ?? v
                                   case .board(let b): return boardName(b)
                                   case .boardList: return "All boards"
                                   case .view(let v): return viewTitle(v)
                                   case .label(let l): return l
                                   case .release(let r):
                                       guard let d = r.firstIndex(of: "-") else { return r }
                                       return r[..<d] + " " + r[r.index(after: d)...]
                                   }
                               },
                               tip: { [self] k in
                                   switch pin(k) {
                                   case .myWork: return "From the issues already synced (no request)"
                                   case .board(let b): return boardTip(b)
                                   case .boardList: return "Every board in the projects in scope — ☆ one to pin it here"
                                   case .view(let v): return "\(viewTitle(v)) — \(v.split(separator: "|").last == "table" ? "table" : "columns")"
                                   case .label(let l): return "Show every issue labelled \(l) here"
                                   case .release(let r): return "Show every issue in \(r) here"
                                   }
                               },
                               menu: { [self] k in
                                   let m = NSMenu()
                                   switch pin(k) {
                                   case .myWork(let v):
                                       m.addItem(menuItem("Show Issues") { [self] in showMyWorkPin(v) })
                                       m.addItem(menuItem("Hide My Work…") { [self] in
                                           w.showToast("Set [jira] my-work = false in commands.toml", symbol: "info.circle")
                                       })
                                   case .board(let b):
                                       m.addItem(menuItem("Open Board") { [self] in openBoard(b) })
                                       if pinnedBoards.contains(b) {
                                           m.addItem(menuItem("Update Now") { [self] in pollBoard(b) })
                                           m.addItem(.separator())
                                           m.addItem(menuItem("Unpin Board (Stop Updating)") { [self] in setPinnedBoards([b], on: false) })
                                       }
                                   case .boardList:
                                       m.addItem(menuItem("Show All Boards") { [self] in showBoardList() })
                                   case .view(let v):
                                       m.addItem(menuItem("Open") { [self] in openView(v) })
                                       m.addItem(.separator())
                                       m.addItem(menuItem("Unpin") { [self] in setPinnedView(v, on: false) })
                                   case .label(let l):
                                       m.addItem(menuItem("Show Issues") { [self] in showLabelPin(l) })
                                       m.addItem(menuItem("Unpin Label") { [self] in setPinnedLabels([l], on: false) })
                                   case .release(let r):
                                       m.addItem(menuItem("Show Issues") { [self] in showReleasePin(r) })
                                       m.addItem(menuItem("Unfavorite Release") { [self] in setReleaseFavoriteKeys([r], on: false) })
                                   }
                                   return m
                               },
                               section: { [self] k in
                                   switch pin(k) {
                                   case .myWork: return "My work"
                                   case .view, .label, .release: return "Pinned"
                                   case .board, .boardList: return "Boards"
                                   }
                               },
                               iconFor: { [self] k in
                                   switch pin(k) {
                                   case .myWork(let v): return ListSession.myWork.first { $0.0 == v }?.2 ?? "person"
                                   case .board: return "rectangle.split.3x1"
                                   case .boardList: return "square.grid.2x2"
                                   case .view(let v): return v.hasSuffix("|table") ? "tablecells" : "rectangle.split.3x1.fill"
                                   case .label: return "tag"
                                   case .release: return "shippingbox"
                                   }
                               },
                               meta: { [self] k in
                                   switch pin(k) {
                                   case .view: return "Board"
                                   case .label: return "Label"
                                   case .release: return "Release"
                                   case .boardList: return catalog.isEmpty ? nil : "\(catalog.count)"
                                   case .myWork, .board: return nil
                                   }
                               },
                               maxShown: 60,
                               onClick: { [self] k in
                                   backToBoardList = false
                                   switch pin(k) {
                                   case .myWork(let v): showMyWorkPin(v)
                                   case .board(let b): openBoard(b)
                                   case .boardList: showBoardList()
                                   case .view(let v): openView(v)
                                   case .label(let l): showLabelPin(l)
                                   case .release(let r): showReleasePin(r)
                                   }
                               })
            refreshBoardList()
        }

        func showPinFile(_ key: String, _ path: String, what: String) {
            let cols = ListSession.tabColumns(cmd, path)
            pinView = (key, path, host.loadListItems(path, cmd: cmd, columns: cols))
            cacheStamp = mtime(of: JiraPoll.issueCachePath)
            pinStamp = mtime(of: path)
            w.selectSidebarPin(key)
            showRows(columns: cols)
            host.log("list '\(cmd.name)': \(what) (\(pinView?.items.count ?? 0) issues)")
        }

        func showMyWorkPin(_ id: String) {
            JiraPoll.run("jira_poll.py", ["--my-work"]) { [weak self] code, out, err in
                guard let self else { return }
                guard code == 0, let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any],
                      let dir = d["dir"] as? String else {
                    self.host.log("jira: my work failed (exit \(code)) \(err)")
                    self.w.showToast("My work: \(JiraPoll.errorLine(err, fallback: "failed"))", symbol: "exclamationmark.triangle")
                    return
                }
                let path = (dir as NSString).appendingPathComponent("\(id).json")
                guard id == "watching", !FileManager.default.fileExists(atPath: path) else {
                    self.showPinFile(ListSession.myWorkPrefix + id, path, what: "my work -> \(id)")
                    return
                }
                self.w.showToast("Fetching the issues you watch…", symbol: "eye")
                self.pollJob("mywork-watching") { [weak self] in
                    guard let self, FileManager.default.fileExists(atPath: path) else { return }
                    self.showPinFile(ListSession.myWorkPrefix + id, path, what: "my work -> watching")
                }
            }
        }

        func showBoardPin(_ id: String) {
            let path = (JiraPoll.sideDir(JiraPoll.boardDir) as NSString).appendingPathComponent("board-\(id).json")
            if FileManager.default.fileExists(atPath: path) {
                showPinFile(ListSession.boardPinPrefix + id, path, what: "board -> \(id)")
                return
            }
            w.showToast("Fetching \(boardName(id))…", symbol: "arrow.down.circle")
            pollBoard(id) { [weak self] in
                guard let self, FileManager.default.fileExists(atPath: path) else { return }
                self.showPinFile(ListSession.boardPinPrefix + id, path, what: "board -> \(id)")
            }
        }

        func pollBoard(_ id: String, then: (() -> Void)? = nil) { pollJob("board-\(id)", then: then) }

        func pollJob(_ name: String, then: (() -> Void)? = nil) {
            JiraPoll.run("jira_poll.py", ["--force", "--quiet", "--projects", name]) { [weak self] code, _, err in
                guard let self else { return }
                self.host.log("jira: poll \(name) (exit \(code))" + (code == 0 ? "" : " " + err))
                if code != 0 {
                    self.w.showToast("Not fetched: \(JiraPoll.errorLine(err, fallback: "see poll.log"))",
                                     symbol: "exclamationmark.triangle")
                }
                then?()
            }
        }

        func boardMode(_ b: String) -> String {
            UserDefaults.standard.string(forKey: "jiraBoardMode.\(b)") ?? "columns"
        }
        func catalogBoard(_ b: String) -> JiraPoll.CatalogBoard? { catalog.first { $0.id == b } }
        func defaultSprint(_ b: String) -> String {
            catalogBoard(b)?.sprints.contains { $0.state == "active" } == true ? "current" : "all"
        }
        func sprintTitle(_ b: String, _ sp: String) -> String {
            switch sp {
            case "all": return "All issues"
            case "current":
                return catalogBoard(b)?.sprints.first { $0.state == "active" }.map { "\($0.name) (current)" } ?? "Current sprint"
            default: return catalogBoard(b)?.sprints.first { $0.id == sp }?.name ?? "Sprint \(sp)"
            }
        }
        func viewTitle(_ v: String) -> String {
            let p = v.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
            guard p.count == 3 else { return v }
            return p[1] == "all" ? boardName(p[0]) : "\(boardName(p[0])) · \(sprintTitle(p[0], p[1]))"
        }
        func boardTip(_ b: String) -> String {
            let c = catalogBoard(b)
            let kind = c?.type == "simple" ? "team-managed board" : "\(c?.type ?? "") board"
            let sprints = c.map { $0.sprints.isEmpty
                ? ($0.type == "kanban" ? "no sprints (kanban)" : "no sprints (Sprints are off for it in Jira)")
                : "\($0.sprints.count) sprints" } ?? ""
            return [c?.project ?? "", kind, sprints].filter { !$0.isEmpty }.joined(separator: " · ")
        }
        var currentViewSpec: String? {
            onBoard.map { "\($0)|\(boardSprint[$0] ?? defaultSprint($0))|\(boardMode($0))" }
        }
        var sidebarSelection: String? {
            if let v = currentViewSpec, pinnedViews.contains(v) { return ListSession.viewPinPrefix + v }
            return pinView?.key
        }

        func openBoard(_ b: String, sprint: String? = nil, mode: String? = nil) {
            if let sp = sprint { boardSprint[b] = sp } else if boardSprint[b] == nil { boardSprint[b] = defaultSprint(b) }
            if let m = mode { UserDefaults.standard.set(m, forKey: "jiraBoardMode.\(b)") }
            if !pinnedBoards.contains(b) {
                setPinnedBoards([b], on: true) { [weak self] in
                    guard let self, self.pinnedBoards.contains(b) else { return }
                    self.showBoardPin(b)
                }
                return
            }
            if boardSprintOnly(b) { setBoardSprint(b, false) }
            showBoardPin(b)
        }

        func openView(_ v: String) {
            let p = v.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
            guard p.count == 3 else { return }
            openBoard(p[0], sprint: p[1], mode: p[2])
        }

        func setPinnedView(_ v: String, on: Bool) {
            pinnedViews = on ? pinnedViews + [v] : pinnedViews.filter { $0 != v }
            syncReleasePins()
            updateBoardChrome()
            JiraPoll.run("jira_poll.py", ["--pin-view", on ? "add" : "remove", v]) { [weak self] code, _, err in
                guard let self else { return }
                if code != 0 {
                    self.w.showToast("Not saved: \(JiraPoll.errorLine(err, fallback: "error"))", symbol: "exclamationmark.triangle")
                }
                self.pinnedViews = JiraPoll.pinnedViews()
                self.syncReleasePins()
            }
            w.showToast(on ? "Pinned \(viewTitle(v))" : "Unpinned", symbol: on ? "pin.fill" : "pin.slash")
        }

        func updateBoardChrome() {
            guard isJira, let b = onBoard else {
                if boardBar != nil { w.setListBar(nil); boardBar = nil }
                if boardCols != nil { w.setListOverlay(nil); boardCols = nil }
                sprintKeys = nil
                w.onRowsChanged = nil
                return
            }
            let sp = boardSprint[b] ?? defaultSprint(b)
            let bar = boardBar ?? {
                let v = JiraBoardBar(frame: .zero)
                v.applyColors(w.config.colors)
                boardBar = v
                w.setListBar(v, height: JiraBoardBar.height)
                return v
            }()
            bar.onSprint = { [weak self] id in self?.pickSprint(id) }
            bar.onMode = { [weak self] m in
                guard let self, let b = self.onBoard else { return }
                UserDefaults.standard.set(m, forKey: "jiraBoardMode.\(b)")
                self.updateBoardChrome()
            }
            bar.onPin = { [weak self] in
                guard let self, let v = self.currentViewSpec else { return }
                self.setPinnedView(v, on: !self.pinnedViews.contains(v))
            }
            var choices: [JiraBoardBar.SprintChoice] = []
            let sprints = catalogBoard(b)?.sprints ?? []
            if !sprints.isEmpty {
                if sprints.contains(where: { $0.state == "active" }) {
                    choices.append(.init(id: "current", title: sprintTitle(b, "current"), header: false))
                }
                for s in sprints where s.state == "future" {
                    choices.append(.init(id: s.id, title: "\(s.name) (next)", header: false))
                }
                let past = sprints.filter { $0.state == "closed" }
                if !past.isEmpty {
                    choices.append(.init(id: "", title: "", header: true))
                    choices.append(.init(id: "", title: "Past sprints", header: true))
                    for s in past { choices.append(.init(id: s.id, title: s.name, header: false)) }
                }
                choices.append(.init(id: "", title: "", header: true))
                choices.append(.init(id: "all", title: "All issues", header: false))
            }
            let mode = boardMode(b)
            let n = w.rows.filter { !(($0 as? FieldRow)?.synthetic ?? false) }.count
            bar.show(board: boardName(b), choices: choices, picked: sp, mode: mode,
                     pinned: currentViewSpec.map(pinnedViews.contains) ?? false,
                     summary: sprintSummary(b, sp, count: n))
            if mode == "columns" {
                if boardCols == nil {
                    let v = JiraBoardColumnsView(frame: .zero)
                    v.onOpen = { [weak self] key in self?.openIssue(key) }
                    boardCols = v
                    w.setListOverlay(v)
                }
                w.onRowsChanged = { [weak self] in self?.renderBoardColumns() }
                renderBoardColumns()
            } else {
                if boardCols != nil { w.setListOverlay(nil); boardCols = nil }
                w.onRowsChanged = { [weak self] in self?.refreshBoardSummary() }
            }
            w.selectSidebarPin(sidebarSelection ?? ListSession.boardPinPrefix + b)
            if sp != "all" {
                let ck = "\(b)|\(sp)"
                if let hit = sprintKeyCache[ck], Date().timeIntervalSince(hit.at) < 300 {
                    if sprintKeys != hit.keys { sprintKeys = hit.keys; w.setRows(filteredRows(query: w.currentQuery)) }
                } else {
                    fetchSprintKeys(b, sp)
                }
            } else if sprintKeys != nil {
                sprintKeys = nil
                w.setRows(filteredRows(query: w.currentQuery))
            }
        }

        func pickSprint(_ id: String) {
            guard let b = onBoard else { return }
            boardSprint[b] = id
            updateBoardChrome()
        }

        private func fetchSprintKeys(_ b: String, _ sp: String) {
            JiraPoll.run("jira_poll.py", ["--board-sprint-keys", b, sp]) { [weak self] code, out, err in
                guard let self else { return }
                let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
                guard code == 0, let keys = d["keys"] as? [String] else {
                    self.host.log("jira: sprint keys \(b) \(sp) failed (exit \(code)) \(err)")
                    self.w.showToast("Couldn't load the sprint: \((d["problems"] as? [String])?.first ?? JiraPoll.errorLine(err, fallback: "error"))",
                                     symbol: "exclamationmark.triangle")
                    return
                }
                self.sprintKeyCache["\(b)|\(sp)"] = (Set(keys), Date())
                guard self.onBoard == b, (self.boardSprint[b] ?? self.defaultSprint(b)) == sp else { return }
                self.sprintKeys = Set(keys)
                self.w.setRows(self.filteredRows(query: self.w.currentQuery))
            }
        }

        func sprintSummary(_ b: String, _ sp: String, count: Int) -> String {
            var parts = ["\(count) issue\(count == 1 ? "" : "s")"]
            let s = sp == "current" ? catalogBoard(b)?.sprints.first { $0.state == "active" }
                                    : catalogBoard(b)?.sprints.first { $0.id == sp }
            func day(_ v: String) -> String? {
                guard v.count >= 10 else { return nil }
                let f = DateFormatter()
                f.locale = Locale(identifier: "en_US_POSIX")
                f.dateFormat = "yyyy-MM-dd"
                guard let d = f.date(from: String(v.prefix(10))) else { return nil }
                f.dateFormat = "MMM d"
                return f.string(from: d)
            }
            if let s {
                if s.state == "closed", let e = day(s.complete) ?? day(s.end) { parts.append("ended \(e)") }
                else if let a = day(s.start) { parts.append("\(a) – \(day(s.end) ?? "?")") }
            }
            return parts.joined(separator: " · ")
        }

        private func refreshBoardSummary() {
            guard let b = onBoard, let bar = boardBar else { return }
            let sp = boardSprint[b] ?? defaultSprint(b)
            let n = w.rows.filter { !(($0 as? FieldRow)?.synthetic ?? false) }.count
            bar.setSummary(sprintSummary(b, sp, count: n))
        }

        private func renderBoardColumns() {
            guard let b = onBoard, let view = boardCols else { return }
            let rows = filteredItems(query: w.currentQuery)
            let cats = statusCats ?? JiraDirectory.load().statusCategories
            statusCats = cats
            var cols = (boards[b]?.columns ?? []).map { (name: $0.name, statuses: Set($0.statuses), cards: [JiraBoardCard]()) }
            if cols.isEmpty { cols = JiraTicketPage.categoryNames.map { ($0, [], []) } }
            var other: [JiraBoardCard] = []
            for r in rows {
                func f(_ k: String) -> String { r.fields[k] ?? "" }
                let st = f("status"), cat = JiraTicketPage.category(of: st, in: cats)
                let card = JiraBoardCard(key: f("key"), title: f("title").isEmpty ? r.title : f("title"),
                                         type: f("type"), priority: f("priority"),
                                         assignee: peopleName(f("assignee")), status: st, cat: cat)
                if boards[b]?.columns.isEmpty == false {
                    if let i = cols.firstIndex(where: { $0.statuses.contains(st) }) { cols[i].cards.append(card) }
                    else { other.append(card) }
                } else {
                    cols[cat].cards.append(card)
                }
            }
            var out = cols.map { c -> JiraBoardColumn in
                let doneCol = !c.cards.isEmpty && c.cards.allSatisfy { $0.cat == 2 }
                let keep = doneCol ? Array(c.cards.prefix(30)) : c.cards
                return JiraBoardColumn(name: c.name, cards: keep, more: c.cards.count - keep.count)
            }
            if !other.isEmpty { out.append(JiraBoardColumn(name: "Not on the board", cards: other, more: 0)) }
            view.show(out, colors: w.config.colors)
            refreshBoardSummary()
        }

        private func peopleName(_ v: String) -> String {
            guard !v.isEmpty else { return "" }
            return peopleByValue()[v]?.title ?? v
        }

        private func openIssue(_ key: String) {
            guard let row = currentItems().first(where: { $0.fields["key"] == key }) else { return }
            host.openRow(row, cmd: cmd, isJira: isJira)
        }

        func boardSprintOnly(_ id: String) -> Bool {
            JiraPoll.endpoints.first { ($0["name"] as? String) == "board-\(id)" }?["sprintOnly"] as? Bool ?? false
        }

        func setBoardSprint(_ id: String, _ on: Bool) {
            JiraPoll.run("jira_poll.py", ["--board-sprint", id, on ? "on" : "off"]) { [weak self] code, _, err in
                guard let self else { return }
                guard code == 0 else {
                    self.w.showToast("Not changed: \(JiraPoll.errorLine(err, fallback: "error"))", symbol: "exclamationmark.triangle")
                    return
                }
                self.w.showToast(on ? "Open sprints only" : "Whole board", symbol: "rectangle.split.3x1")
                self.pollBoard(id) { [weak self] in
                    if self?.pinView?.key == ListSession.boardPinPrefix + id { self?.showBoardPin(id) }
                }
            }
        }

        func setPinnedBoards(_ ids: [String], on: Bool, then: (() -> Void)? = nil) {
            guard !ids.isEmpty else { then?(); return }
            if on { w.showToast("Reading \(ids.count == 1 ? boardName(ids[0]) : "\(ids.count) boards")…", symbol: "arrow.down.circle") }
            JiraPoll.run("jira_poll.py", ["--pin-board", on ? "add" : "remove"] + ids) { [weak self] code, out, err in
                defer { then?() }
                guard let self else { return }
                let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
                let probs = d["problems"] as? [String] ?? []
                self.host.log("jira: pin board \(on ? "add" : "remove") \(ids.joined(separator: ",")) (exit \(code)) \(probs) \(err)")
                self.pinnedBoards = JiraPoll.pinnedBoards()
                self.boards = JiraPoll.boardsInfo()
                self.syncReleasePins()
                if code != 0 {
                    self.w.showToast("Board not pinned: \(probs.first ?? JiraPoll.errorLine(err, fallback: "error"))",
                                     symbol: "exclamationmark.triangle")
                    return
                }
                if on {
                    self.w.showToast("Pinned \(ids.count == 1 ? self.boardName(ids[0]) : "\(ids.count) boards")", symbol: "pin.fill")
                    for b in ids where self.pinnedBoards.contains(b) { self.pollBoard(b) }
                } else {
                    self.w.showToast("Unpinned", symbol: "pin.slash")
                }
            }
        }

        func showQuickFilterPicker(anchor: NSView, rect: NSRect) {
            guard let b = onBoard, let qf = boards[b]?.quickFilters, !qf.isEmpty else { return }
            openPicker?.closePopover()
            let p = JiraMultiPicker(noun: "quick filter")
            p.applyColors(w.config.colors)
            p.options = qf.map { .init(id: $0.id, title: $0.name, detail: $0.jql) }
            p.set(Array(colFilters[ListSession.quickDim] ?? []).sorted())
            p.anchor = (anchor, rect)
            p.onClose = { [self, weak p] in
                openPicker = nil
                guard let p else { return }
                let ids = p.selected
                guard Set(ids) != (colFilters[ListSession.quickDim] ?? []) else { return }
                colFilters[ListSession.quickDim] = ids.isEmpty ? nil : Set(ids)
                updateFilterIndicators()
                guard !ids.isEmpty else {
                    quickKeys = nil
                    w.setRows(filteredRows(query: w.currentQuery))
                    return
                }
                w.showToast("Applying quick filters…", symbol: "line.3.horizontal.decrease.circle")
                JiraPoll.run("jira_poll.py", ["--board-quickfilter", b] + ids) { [weak self] code, out, err in
                    guard let self, self.onBoard == b else { return }
                    let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
                    guard code == 0, let keys = d["keys"] as? [String] else {
                        self.colFilters[ListSession.quickDim] = nil
                        self.updateFilterIndicators()
                        self.w.showToast("Quick filter failed: \((d["problems"] as? [String])?.first ?? JiraPoll.errorLine(err, fallback: "error"))",
                                         symbol: "exclamationmark.triangle")
                        return
                    }
                    self.quickKeys = Set(keys)
                    self.visibleOffset = 0
                    self.w.setRows(self.filteredRows(query: self.w.currentQuery))
                }
            }
            openPicker = p
            p.togglePopover(nil)
        }

        func issueLabels(_ rows: [FieldRow]) -> [String] {
            var out: [String] = []
            for r in rows {
                for l in (r.fields["labels"] ?? "").split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) })
                    where !l.isEmpty && !out.contains(l) { out.append(l) }
            }
            return out
        }

        func importFilters() {
            w.showToast("Importing your favourite filters…", symbol: "line.3.horizontal.decrease.circle")
            JiraPoll.run("jira_poll.py", ["--import-filters"]) { [weak self] code, out, err in
                guard let self else { return }
                let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
                guard code == 0 else {
                    self.w.showToast("Import failed: \((d["problems"] as? [String])?.first ?? JiraPoll.errorLine(err, fallback: "error"))",
                                     symbol: "exclamationmark.triangle")
                    return
                }
                let n = (d["filters"] as? [Any])?.count ?? 0, added = d["added"] as? Int ?? 0
                self.w.showToast(n == 0 ? "No favourite filters on Jira" : "\(n) filter(s): \(added) new tab(s) on the next poll",
                                 symbol: "line.3.horizontal.decrease.circle")
                if added > 0 { JiraPoll.run("jira_poll.py", ["--force", "--quiet"]) }
                self.host.reloadJiraWindow()
            }
        }

        func showLabelPin(_ name: String) {
            let key = ListSession.labelPinPrefix + name
            JiraPoll.run("jira_poll.py", ["--label-view", name]) { [weak self] code, out, err in
                guard let self else { return }
                guard code == 0, let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any],
                      let dir = d["dir"] as? String, let file = d["file"] as? String else {
                    self.host.log("jira: label pin \(name) failed (exit \(code)) \(err)")
                    self.w.showToast("No issues found for \(name)", symbol: "exclamationmark.triangle")
                    return
                }
                self.showPinFile(key, (dir as NSString).appendingPathComponent(file), what: "label pin -> \(name)")
            }
        }

        func setPinnedLabels(_ names: [String], on: Bool) {
            guard !names.isEmpty else { return }
            let before = pinnedLabels
            pinnedLabels = on ? pinnedLabels + names.filter { !pinnedLabels.contains($0) }
                              : pinnedLabels.filter { !names.contains($0) }
            syncReleasePins()
            let s = names.count == 1 ? names[0] : "\(names.count) labels"
            w.showToast(on ? "Pinned \(s) to the sidebar" : "Unpinned \(s)", symbol: on ? "tag.fill" : "tag")
            JiraPoll.run("jira_poll.py", ["--pin-label", on ? "add" : "remove"] + names) { [self] code, _, err in
                host.log("jira: pin label \(on ? "add" : "remove") \(names.joined(separator: ",")) (exit \(code))"
                         + (code == 0 ? "" : " " + err))
                guard code != 0 else { return }
                pinnedLabels = before
                syncReleasePins()
                w.showToast("Label pin not saved: \(JiraPoll.errorLine(err, fallback: "error"))",
                            symbol: "exclamationmark.triangle")
            }
        }

        func showReleasePin(_ key: String) {
            JiraPoll.run("jira_poll.py", ["--release-view", key]) { [weak self] code, out, err in
                guard let self else { return }
                guard code == 0, let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any],
                      let dir = d["dir"] as? String,
                      let file = (d["releases"] as? [[String: Any]])?
                          .first(where: { ($0["key"] as? String) == key })?["file"] as? String else {
                    self.host.log("jira: release pin \(key) failed (exit \(code)) \(err)")
                    self.w.showToast("No issues found for \(key)", symbol: "exclamationmark.triangle")
                    return
                }
                let path = (dir as NSString).appendingPathComponent(file)
                let cols = ListSession.tabColumns(self.cmd, path)
                self.pinView = (key, path, self.host.loadListItems(path, cmd: self.cmd, columns: cols))
                self.cacheStamp = mtime(of: JiraPoll.issueCachePath)
                self.w.selectSidebarPin(key)
                self.showRows(columns: cols)
                self.host.log("list '\(self.cmd.name)': release pin -> \(key) (\(self.pinView?.items.count ?? 0) issues)")
            }
        }

        func leavePinView() {
            guard pinView != nil else { return }
            backToBoardList = false
            pinView = nil
            w.clearSidebarPin()
            w.selectedTab = currentTab
            showRows(columns: ListSession.tabColumns(cmd, tabs[currentTab].path))
        }

        func showRows(columns cols: [ListColumn]) {
            invalidateFilter()
            visibleOffset = 0
            if cmd.table {
                if cols.map(\.field) != columns.map(\.field) || cols.map(\.width) != columns.map(\.width)
                    || cols.map(\.title) != columns.map(\.title) {
                    columns = cols
                    w.setTableColumns(cols.map { $0.popup })
                }
                if let k = sortKey, !columns.contains(where: { $0.field == k.field }) { sortKey = nil }
                syncSortArrow()
            }
            w.clearInput()
            openPicker?.closePopover()
            colFilters = [:]
            quickKeys = nil
            applyFilterData()
            sprintKeys = onBoard.flatMap { b in sprintKeyCache["\(b)|\(boardSprint[b] ?? "")"]?.keys }
            w.setRows(filteredRows(query: ""))
            w.tabFooterText = ""
            updateBoardChrome()
        }

        func setReleaseFavorite(_ rows: [FieldRow], on: Bool) {
            setReleaseFavoriteKeys(rows.compactMap { $0.fields["key"] }.filter { !$0.isEmpty }, on: on)
        }

        func setReleaseFavoriteKeys(_ keys: [String], on: Bool) {
            guard !keys.isEmpty else { return }
            let before = favReleases
            favReleases = on ? keys.filter { !favReleases.contains($0) } + favReleases
                             : favReleases.filter { !keys.contains($0) }
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            syncReleasePins()
            let s = keys.count == 1 ? keys[0] : "\(keys.count) releases"
            w.showToast(on ? "Starred \(s)" : "Unstarred \(s)", symbol: on ? "star.fill" : "star")
            JiraPoll.run("jira_poll.py", ["--favorite-release", on ? "add" : "remove"] + keys) { [self] code, _, err in
                host.log("jira: favorite release \(on ? "add" : "remove") \(keys.joined(separator: ",")) (exit \(code))"
                         + (code == 0 ? "" : " " + err))
                guard code != 0 else { return }
                favReleases = before
                w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
                syncReleasePins()
                w.showToast("Release star not saved: \(JiraPoll.errorLine(err, fallback: "error"))",
                            symbol: "exclamationmark.triangle")
            }
        }

        func setBlacklisted(_ rows: [FieldRow], on: Bool) {
            let keys = rows.compactMap { $0.fields["key"] }.filter { !$0.isEmpty }
            guard !keys.isEmpty else { return }
            JiraPoll.run("jira_poll.py", ["--blacklist-release", on ? "add" : "remove"] + keys) {
                [self] code, _, err in
                host.log("jira: blacklist \(on ? "add" : "remove") \(keys.joined(separator: ",")) (exit \(code))"
                         + (code == 0 ? "" : " " + err))
                let s = keys.count == 1 ? "release \(rows[0].title)" : "\(keys.count) releases"
                guard code == 0 else {
                    w.showToast("Not saved: \(JiraPoll.errorLine(err, fallback: "error"))",
                                symbol: "exclamationmark.triangle")
                    return
                }
                w.selectedIndices = []
                w.showToast(on ? "Hid \(s) → \(JiraPoll.blacklistFile)" : "Restored \(s)",
                            symbol: on ? "eye.slash" : "eye")
                ensureTab(JiraPoll.blacklistFile)
            }
        }

        func invalidateFilter() {
            filterIndex = nil
            sortRanks = nil
            filterGen += 1
            guard !filterPrewarmQueued else { return }
            filterPrewarmQueued = true
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                filterPrewarmQueued = false
                guard tabs.indices.contains(currentTab) else { return }
                let gen = filterGen, key = sortKey, items = currentItems()
                DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                    let index = FuzzyIndex(items.map(\.searchText))
                    let ranks = key.map { k in
                        SortRank.ranks(items.map { $0.fields[k.field] ?? "" }, ascending: k.ascending)
                    }
                    DispatchQueue.main.async { [weak self] in
                        guard let self, filterGen == gen else { return }
                        if filterIndex == nil { filterIndex = index }
                        if let k = key, let ranks, sortRanks == nil,
                           sortKey?.field == k.field, sortKey?.ascending == k.ascending {
                            sortRanks = (k.field, k.ascending, ranks)
                        }
                    }
                }
            }
        }

        var onBoard: String? {
            guard let k = pinView?.key, k.hasPrefix(ListSession.boardPinPrefix) else { return nil }
            return String(k.dropFirst(ListSession.boardPinPrefix.count))
        }
        var effectiveGroupBy: String? {
            guard isJira, !onBoardList else { return nil }
            if onBoard != nil { return boardGroupBy.isEmpty ? nil : boardGroupBy }
            return groupBy == "boardColumn" ? nil : groupBy
        }

        func groupChoices() -> [(String, String)] {
            var out: [(String, String)] = [("Status category", "statusCategory"), ("Status", "status"),
                                           ("Assignee", "assignee"), ("Priority", "priority"),
                                           ("Release", "release"), ("Labels", "labels"), ("Epic / parent", "epic"),
                                           ("Components", "components"), ("Project", "project")]
            for c in columns where !out.contains(where: { $0.1 == c.field }) && c.filterable
                && !["key", "title", "updated", "created", "description", "releaseLabel", "releaseDate"].contains(c.field) {
                out.append((label(c.field), c.field))
            }
            if onBoard != nil { out.insert(("Board column", "boardColumn"), at: 0) }
            return out
        }

        func setGroupBy(_ g: String?) {
            collapsedGroups = []
            if onBoard != nil {
                boardGroupBy = g ?? ""
            } else {
                groupBy = g
                UserDefaults.standard.set(g, forKey: "listGroupBy.\(cmd.name)")
            }
            w.setRows(filteredRows(query: w.currentQuery))
            w.showToast(g.map { v in "Grouped by \(groupChoices().first { $0.1 == v }?.0 ?? v)" } ?? "Not grouped",
                        symbol: "rectangle.grid.1x2")
        }

        func groupMenuItems() -> [NSMenuItem] {
            guard isJira else { return [] }
            let sub = NSMenu()
            let cur = effectiveGroupBy
            sub.addItem(menuItem("None", state: cur == nil) { [self] in setGroupBy(nil) })
            sub.addItem(.separator())
            for (t, v) in groupChoices() {
                sub.addItem(menuItem(t, state: cur == v) { [self] in setGroupBy(v) })
            }
            if cur != nil {
                sub.addItem(.separator())
                sub.addItem(menuItem("Collapse All") { [self] in toggleAllGroups(collapse: true) })
                sub.addItem(menuItem("Expand All") { [self] in toggleAllGroups(collapse: false) })
            }
            let item = NSMenuItem(title: "Group By", action: nil, keyEquivalent: "")
            item.submenu = sub
            return [item]
        }

        func toggleGroup(_ name: String) {
            if collapsedGroups.contains(name) { collapsedGroups.remove(name) } else { collapsedGroups.insert(name) }
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
        }

        func toggleAllGroups(collapse: Bool) {
            collapsedGroups = collapse ? Set(w.rows.compactMap { ($0 as? FieldRow)?.fields["__group"] }) : []
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
        }

        func groupedRows(_ order: [Int], items: [FieldRow], by g: String) -> [FieldRow] {
            let none = "(none)"
            var fixed: [String] = []
            var keyOf: (FieldRow) -> [String]
            switch g {
            case "statusCategory":
                if statusCats == nil { statusCats = JiraDirectory.load().statusCategories }
                let cats = statusCats ?? [:], names = JiraTicketPage.categoryNames
                var memo: [String: String] = [:]
                fixed = names
                keyOf = { r in
                    let st = r.fields["status"] ?? ""
                    if st.isEmpty { return [none] }
                    if let m = memo[st] { return [m] }
                    let c = names[JiraTicketPage.category(of: st, in: cats)]
                    memo[st] = c
                    return [c]
                }
            case "boardColumn":
                let cols = onBoard.flatMap { boards[$0]?.columns } ?? []
                var col: [String: String] = [:]
                for c in cols { for st in c.statuses where col[st] == nil { col[st] = c.name } }
                fixed = cols.map(\.name)
                keyOf = { r in [col[r.fields["status"] ?? ""] ?? "Not on the board"] }
            default:
                let people = ListSession.personFields.contains(g) ? peopleByValue() : [:]
                keyOf = { [self] r in
                    cellValues(g, r).map { v in
                        v == ListSession.emptyValue ? (g == "assignee" ? "Unassigned" : none) : (people[v]?.title ?? v)
                    }
                }
            }
            var groups: [String: [Int]] = [:]
            var seen: [String] = []
            for i in order {
                for k in keyOf(items[i]) {
                    if groups[k] == nil { seen.append(k) }
                    groups[k, default: []].append(i)
                }
            }
            let tail: Set<String> = [none, "Unassigned", "Not on the board"]
            let rest = seen.filter { !fixed.contains($0) }.sorted { a, b in
                if tail.contains(a) != tail.contains(b) { return tail.contains(b) }
                let ca = groups[a]?.count ?? 0, cb = groups[b]?.count ?? 0
                return ca != cb ? ca > cb : a.localizedStandardCompare(b) == .orderedAscending
            }
            var out: [FieldRow] = []
            for name in fixed.filter({ groups[$0] != nil }) + rest {
                let idx = groups[name] ?? []
                let shut = collapsedGroups.contains(name)
                out.append(FieldRow(title: name, content: shut ? "1" : "", trailing: "\(idx.count)",
                                    detail: nil, body: nil, searchText: "", fields: ["__group": name]))
                guard !shut else { continue }
                for i in idx {
                    var r = items[i]
                    if let k = r.fields["key"], !k.isEmpty {
                        r.starred = jiraIsReleaseRow(r) ? favReleases.contains(k) : favKeys.contains(k)
                    }
                    out.append(r)
                }
            }
            return out
        }

        func filteredItems(query: String) -> [FieldRow] {
            let items = currentItems()
            let index = filterIndex ?? FuzzyIndex(items.map(\.searchText))
            filterIndex = index
            var order = index.ranked(query)
            if let qk = quickKeys { order = order.filter { qk.contains(items[$0].fields["key"] ?? "") } }
            if onBoard != nil, let sk = sprintKeys { order = order.filter { sk.contains(items[$0].fields["key"] ?? "") } }
            let active = colFilters.filter { !$0.value.isEmpty && $0.key != ListSession.quickDim }
            if !active.isEmpty {
                order = order.filter { i in
                    active.allSatisfy { f, picked in cellValues(f, items[i]).contains(where: picked.contains) }
                }
            }
            if let k = sortKey {
                if sortRanks?.field != k.field || sortRanks?.ascending != k.ascending {
                    sortRanks = (k.field, k.ascending,
                                 SortRank.ranks(items.map { $0.fields[k.field] ?? "" }, ascending: k.ascending))
                }
                order = SortRank.order(order, by: sortRanks?.ranks ?? [])
            }
            return order.map { items[$0] }
        }

        func filteredRows(query: String) -> [FieldRow] {
            let t0 = DispatchTime.now().uptimeNanoseconds
            let items = currentItems()
            let index = filterIndex ?? FuzzyIndex(items.map(\.searchText))
            filterIndex = index
            var order = index.ranked(query)
            let t1 = DispatchTime.now().uptimeNanoseconds
            let active = colFilters.filter { !$0.value.isEmpty && $0.key != ListSession.quickDim }
            if let qk = quickKeys {
                order = order.filter { qk.contains(items[$0].fields["key"] ?? "") }
            }
            if onBoard != nil, let sk = sprintKeys {
                order = order.filter { sk.contains(items[$0].fields["key"] ?? "") }
            }
            if !active.isEmpty {
                order = order.filter { i in
                    for (f, picked) in active where !cellValues(f, items[i]).contains(where: picked.contains) {
                        return false
                    }
                    return true
                }
            }
            if let k = sortKey {
                if sortRanks?.field != k.field || sortRanks?.ascending != k.ascending {
                    sortRanks = (k.field, k.ascending,
                                 SortRank.ranks(items.map { $0.fields[k.field] ?? "" }, ascending: k.ascending))
                }
                order = SortRank.order(order, by: sortRanks?.ranks ?? [])
            }
            let t2 = DispatchTime.now().uptimeNanoseconds
            defer {
                let total = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1e6
                if total > 8 {
                    host.log(String(format: "list '%@' filter: %d rows -> %d, q=\"%@\", match %.1f ms, "
                                    + "filter+sort %.1f ms, total %.1f ms", cmd.name, items.count, order.count,
                                    query, Double(t1 - t0) / 1e6, Double(t2 - t1) / 1e6, total))
                }
            }
            if let g = effectiveGroupBy {
                let rows = groupedRows(Array(order.prefix(cap)), items: items, by: g)
                let n = rows.filter { !$0.groupHeader }.count
                w.itemCount = n == 1 ? "1 item" : "\(n) items"
                return rows
            }
            let shown = order.prefix(cap)
            var take = shown.count
            if cmd.pageSize > 0, shown.count > cmd.pageSize, query.isEmpty {
                take = min(shown.count, visibleOffset + cmd.pageSize)
            }
            var paged = shown.prefix(take).map { i -> FieldRow in
                var r = items[i]
                if isJira, let k = r.fields["key"], !k.isEmpty {
                    r.starred = jiraIsReleaseRow(r) ? favReleases.contains(k) : favKeys.contains(k)
                }
                return r
            }
            if take < shown.count {
                paged.append(FieldRow(title: "load \(shown.count - take) more…",
                                      content: nil, trailing: nil, detail: nil,
                                      body: nil, searchText: "",
                                      fields: ["__loadmore": "1"]))
            }
            w.itemCount = paged.count == 1 ? "1 item" : "\(paged.count) items"
            return paged
        }

        func setSort(_ f: String, ascending: Bool) {
            sortKey = (f, ascending)
            syncSortArrow()
            visibleOffset = 0
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            let v = "\(f):\(ascending ? "asc" : "desc")"
            guard !isReleaseView, !onBoardList else { return }
            if let ci = host.commands.firstIndex(where: { $0.name == cmd.name }) {
                host.commands[ci].tableSort = v
            }
            saveConfigValue(section: cmd.name, key: "table-sort", value: v)
            host.log("list '\(cmd.name)': sort -> \(v)")
        }

        func showFilterPicker(_ field: String, anchor: NSView, rect: NSRect) {
            openPicker?.closePopover()
            var opts = filterOptions(field)
            let p = JiraMultiPicker(noun: label(field).lowercased())
            p.applyColors(w.config.colors)
            if field == "status" {
                let cats = JiraDirectory.load().statusCategories, names = JiraTicketPage.categoryNames
                var rows: [String: Int] = [:]
                opts = opts.map { o in
                    let g = o.id == ListSession.emptyValue ? "" : names[JiraTicketPage.category(of: o.id, in: cats)]
                    rows[g, default: 0] += Int(o.detail.split(separator: " ").first ?? "") ?? 0
                    return .init(id: o.id, title: o.title, detail: o.detail, group: g)
                }
                p.groupOrder = names
                p.groupDetail = rows.mapValues { "\($0) row\($0 == 1 ? "" : "s")" }
            }
            if field == "labels" {
                opts = JiraMultiPicker.foldTail(opts)
                p.foldTitle = { [weak p] n in "Show all \(p?.options.count ?? n) labels" }
                if cmd.name == "jira" {
                    p.extraButtons.append(("Pin to Sidebar", { [self, weak p] in
                        guard let p else { return }
                        let picked = p.selected.filter { $0 != ListSession.emptyValue }
                        guard !picked.isEmpty else {
                            w.showToast("Tick the labels to pin first", symbol: "tag")
                            return
                        }
                        setPinnedLabels(picked, on: true)
                        p.closePopover()
                    }))
                }
            }
            p.options = opts
            p.set((colFilters[field] ?? []).filter { v in opts.contains { $0.id == v } }.sorted())
            p.anchor = (anchor, rect)
            if let col = columns.first(where: { $0.field == field }), col.sortable {
                p.extraButtons = [
                    ("Sort ↑", { [self, weak p] in setSort(field, ascending: true); p?.closePopover() }),
                    ("Sort ↓", { [self, weak p] in setSort(field, ascending: false); p?.closePopover() }),
                ]
            }
            p.onChange = { [self, weak p] in
                guard let p else { return }
                colFilters[field] = p.selected.isEmpty ? nil : Set(p.selected)
                visibleOffset = 0
                updateFilterIndicators()
                w.setRows(filteredRows(query: w.currentQuery))
            }
            p.onClose = { [self] in openPicker = nil }
            openPicker = p
            p.togglePopover(nil)
        }

        func refreshPathLabel() {
            w.copyPathButtonLabel = ""
        }

        func refreshBadges(force: Bool = false) {
            guard cmd.name == "jira" else { return }
            let st = mtime(of: JiraPoll.statusPath)
            guard force || st != badgeStamp.status || Date().timeIntervalSince(badgeStamp.at) > 30 else { return }
            badgeStamp = (st, Date())
            let status = JiraPoll.status, config = JiraPoll.readJSON(JiraPoll.configPath)
            w.tabBadges = tabs.map { JiraPoll.tabBadge(path: $0.path, status: status, config: config) }
            let sum = JiraPoll.syncSummary(paths: tabs.map(\.path), status: status, config: config)
            w.setSidebarStatus(sum?.text, tone: sum?.tone ?? .success)
        }

        func install() {
            if isJira {
                w.onTestAction = { [weak self] a in
                    guard let self else { return }
                    let p = a.split(separator: ":", maxSplits: 1).map(String.init)
                    switch (p.first ?? "", p.count > 1 ? p[1] : "") {
                    case ("open", let b): self.openBoard(b)
                    case ("list", _): self.showBoardList()
                    case ("list-open", let b): self.openListedBoard(b)
                    case ("list-star", let b): self.setPinnedBoards([b], on: !self.pinnedBoards.contains(b))
                    case ("view", let v): self.openView(v)
                    case ("sprint", let s): self.pickSprint(s)
                    case ("mode", let m): self.boardBar?.onMode?(m)
                    case ("pin", _): self.boardBar?.onPin?()
                    default: break
                    }
                }
                w.testExtra = { [weak self] in
                    guard let self else { return [:] }
                    if self.onBoardList {
                        return ["list": true, "rows": self.pinView?.items.count ?? 0, "pinned": self.pinnedBoards,
                                "selected": self.sidebarSelection ?? ""]
                    }
                    guard let b = self.onBoard else { return [:] }
                    return ["id": b, "sprint": self.boardSprint[b] ?? self.defaultSprint(b), "mode": self.boardMode(b),
                            "columns": self.boardCols != nil, "bar": self.boardBar != nil,
                            "sprintKeys": self.sprintKeys?.count ?? -1, "views": self.pinnedViews,
                            "selected": self.sidebarSelection ?? "", "catalog": self.catalog.map(\.id)]
                }
            }
            w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
            w.headerIcon = jiraAppIcon
            if let ts = cmd.tableSort, !columns.isEmpty {
                let parts = ts.split(separator: ":").map { $0.trimmingCharacters(in: .whitespaces) }
                if let f = parts.first, columns.contains(where: { $0.field == f }) {
                    sortKey = (f, !(parts.count > 1 && parts[1].lowercased().hasPrefix("desc")))
                }
            }
            syncSortArrow()
            let copyKeys = copyKeys
            w.onCopyRows = { picked in
                picked.compactMap { $0 as? FieldRow }
                    .filter { !$0.synthetic }
                    .map { row in
                        copyKeys.map { row.fields[$0] ?? "" }.joined(separator: "\t")
                    }
                    .joined(separator: "\n")
            }
            w.onToggleStar = { [self] i in
                guard w.rows.indices.contains(i), let row = w.rows[i] as? FieldRow,
                      let on = row.starred else { return }
                if let b = row.fields["__board"] { setPinnedBoards([b], on: !on); return }
                if jiraIsReleaseRow(row) { setReleaseFavorite([row], on: !on) } else { setFavorite([row], on: !on) }
            }
            syncReleasePins()
            w.onCommandK = { [self] in showActions() }
            if cmd.name == "jira" { w.onSidebarJump = { [weak host, weak w] in if let w { host?.showSidebarJump(w) } } }
            if isJira { w.onTableHeaderMenu = { [self] in groupMenuItems() } }
            if cmd.name == "jira" {
                w.onCommandF = { [host, weak w] in
                    guard let w else { return }
                    JiraSearchPanel.toggle(on: w, controller: host)
                }
            }
            w.onChromeHeaderClick = { [self] in
                guard tabs.indices.contains(currentTab) else { return }
                host.copy(tabs[currentTab].path, "source path: \(tabs[currentTab].path)")
            }
            if isJira { w.copyConfigButtonLabel = "" }
            w.onChromeConfigClick = { [host] in
                host.copy(settings.commandsConfPath, "config path: \(settings.commandsConfPath)")
            }
            w.onChromeIconClick = { [self] in showIconMenu() }
            w.onShowShortcuts = { [self] in
                host.showShortcuts(on: w, view: cmd.name == "jira" ? "jira" : "")
            }
            applyFilterData()
            w.tabTitles = tabs.map { URL(fileURLWithPath: $0.path).lastPathComponent }
            w.tabPathTip = { [weak self] i in
                guard let self, self.tabs.indices.contains(i) else { return nil }
                return (self.tabs[i].path as NSString).abbreviatingWithTildeInPath
            }
            w.onFilter = { [self] query in
                visibleOffset = 0
                return filteredRows(query: query)
            }
            w.onFilterChange = { [self] _ in
                visibleOffset = 0
                w.setRows(filteredRows(query: w.currentQuery))
            }
            w.onTabChange = { [self] index in selectTab(index) }
            if isJira && cmd.inspectorWidth > 0 {
                w.onSelectionChanged = { [weak w, host] index in
                    guard let w else { return }
                    let row = w.rows.indices.contains(index) ? w.rows[index] as? FieldRow : nil
                    w.inspectorContent = row.flatMap { $0.synthetic || $0.fields["__board"] != nil
                        ? nil : host.jiraInspectorContent($0) }
                }
                w.onInspectorOpen = { [self] in
                    guard w.rows.indices.contains(w.selection), let row = w.rows[w.selection] as? FieldRow,
                          !row.synthetic, row.fields["__board"] == nil else { return }
                    host.openRow(row, cmd: cmd, isJira: isJira)
                }
            }
            w.onTabClick = { [self] index in
                guard index == w.selectedTab, index < tabs.count else { return }
                host.copy(tabs[index].path, "source path: \(tabs[index].path)")
            }
            w.onAccept = { [self] row in
                if let g = (row as? FieldRow)?.fields["__group"] {
                    toggleGroup(g)
                    return
                }
                if row.loadMore {
                    visibleOffset += cmd.pageSize
                    w.setRows(filteredRows(query: w.currentQuery))
                    host.log("list '\(cmd.name)': load more -> offset \(visibleOffset)")
                    return
                }
                guard let row = row as? FieldRow else { return }
                if let b = row.fields["__board"] { openListedBoard(b); return }
                host.log("list '\(cmd.name)': details for '\(row.title)'")
                host.openRow(row, cmd: cmd, isJira: isJira)
            }
            w.onRowClick = { [self] index in
                guard index >= 0, index < w.rows.count else { return }
                if let g = (w.rows[index] as? FieldRow)?.fields["__group"] {
                    toggleGroup(g)
                    return
                }
                if w.rows[index].loadMore {
                    visibleOffset += cmd.pageSize
                    w.setRows(filteredRows(query: w.currentQuery))
                    host.log("list '\(cmd.name)': load more (click) -> offset \(visibleOffset)")
                }
            }
            w.onRowDoubleClick = { [self] index in
                guard index >= 0, index < w.rows.count,
                      let row = w.rows[index] as? FieldRow, !row.synthetic else { return }
                if let b = row.fields["__board"] { openListedBoard(b); return }
                host.openRow(row, cmd: cmd, isJira: isJira)
            }
            w.onTableSort = { [self] i in
                guard columns.indices.contains(i) else { return }
                let f = columns[i].field
                setSort(f, ascending: sortKey?.field == f ? !(sortKey?.ascending ?? true) : true)
            }
            w.onTableFilter = { [self] i, view, rect in
                guard columns.indices.contains(i) else { return }
                showFilterPicker(columns[i].field, anchor: view, rect: rect)
            }
            w.onFilterOpen = { [self] dim, view, rect in
                guard activeDims.indices.contains(dim) else { return }
                if activeDims[dim] == ListSession.quickDim { showQuickFilterPicker(anchor: view, rect: rect); return }
                showFilterPicker(activeDims[dim], anchor: view, rect: rect)
            }
            w.onTableColumnsResized = { [self] pcts, final in columnsResized(pcts, final: final) }
            w.onTableColumnsReordered = { [self] from, to in
                guard columns.indices.contains(from), columns.indices.contains(to) else { return }
                columns.insert(columns.remove(at: from), at: to)
                syncSortArrow()
                updateFilterIndicators()
                w.onTableColumnsResized?(columns.map(\.width), true)
            }
            w.onEscape = { [self] in
                guard inSlot else { w.hide(restore: true); return }
                if !w.currentQuery.isEmpty {
                    w.clearInput()
                    visibleOffset = 0
                    w.setRows(filteredRows(query: ""))
                    return
                }
                if backToBoardList, onBoard != nil { showBoardList(); return }
                if pinView != nil { leavePinView(); return }
                host.slot.back(esc: true)
            }
            if cmd.name == "jira" && inSlot {
                w.onUnpark = { [w] in JiraSearchPanel.unpark(to: w) }
            }
            w.onHide = { [self] restore in
                reloadWatcher?.invalidate()
                reloadWatcher = nil
                if cmd.name == "jira" { JiraSearchPanel.detach(from: w) }
                host.unregisterSubWindow(w, restore: restore,
                                         restoreWID: restoreWID, restorePID: restorePID)
            }
            refreshBadges(force: true)
            let watcher = Timer(timeInterval: listWatchInterval, repeats: true) { [weak self] _ in
                guard let self, self.w.isShown else { return }
                self.watchTick()
            }
            RunLoop.main.add(watcher, forMode: .common)
            reloadWatcher = watcher
            w.copyConfigButtonLabel = ""
            if cmd.table && !columns.isEmpty {
                w.onTableFit = { [weak w] in
                    guard let w, let pcts = w.fitTableColumns() else { return }
                    w.onTableColumnsResized?(pcts, true)
                    w.showToast("Columns fitted to their content", symbol: "arrow.left.and.right")
                }
            }
            refreshPathLabel()
        }

        func didShow() {
            if isReleaseView, let f = host.pendingReleaseTab {
                host.pendingReleaseTab = nil
                if let i = tabs.firstIndex(where: { ($0.path as NSString).lastPathComponent == f }), i != currentTab {
                    w.selectedTab = i
                }
            }
            guard cmd.name == "jira" else { return }
            host.jiraShowTab = { [weak self] file in self?.showTab(file) }
            if let f = host.pendingJiraTab {
                host.pendingJiraTab = nil
                host.jiraShowTab?(f)
            }
            JiraSearchPanel.reattach(to: w)
        }

        private func showTab(_ file: String) {
            guard let i = tabs.firstIndex(where: { ($0.path as NSString).lastPathComponent == file }) else {
                host.pendingJiraTab = file
                host.reloadJiraWindow()
                return
            }
            tabs[i].items = host.loadListItems(tabs[i].path, cmd: cmd, columns: ListSession.tabColumns(cmd, tabs[i].path))
            tabMtimes[i] = mtime(of: tabs[i].path)
            if i != currentTab {
                w.selectedTab = i
            } else {
                applyFilterData()
                visibleOffset = 0
                w.setRows(filteredRows(query: w.currentQuery))
            }
            w.tabFooterText = ""
        }

        private func selectTab(_ index: Int) {
            guard tabs.indices.contains(index), index != currentTab || pinView != nil else { return }
            if pinView != nil {
                pinView = nil
                w.clearSidebarPin()
            }
            currentTab = index
            showRows(columns: ListSession.tabColumns(cmd, tabs[index].path))
            refreshPathLabel()
            host.log("list '\(cmd.name)': tab -> \(tabs[index].path)")
        }

        private func columnsResized(_ pcts: [CGFloat], final: Bool) {
            for i in columns.indices where i < pcts.count { columns[i].width = pcts[i] }
            guard !onBoardList else { return }
            resizeSave?.cancel()
            let item = DispatchWorkItem { [self] in
                let spec = ListColumn.serialize(columns, titles: !isJira)
                if isJira, tabs.indices.contains(currentTab),
                   let own = JiraPoll.owner(ofTab: currentPath) {
                    JiraPoll.run("jira_config.py", ["--set-columns", own.kind, own.name, spec]) { [host] code, _, err in
                        host.log("jira: \(own.kind) \(own.name) columns -> \(spec) (exit \(code))"
                                 + (code == 0 ? "" : " " + err))
                    }
                    return
                }
                if let ci = host.commands.firstIndex(where: { $0.name == cmd.name }) {
                    host.commands[ci].columns = columns
                }
                saveConfigValue(section: cmd.name, key: "columns", value: spec)
                host.log("list '\(cmd.name)': columns -> \(spec)")
            }
            resizeSave = item
            DispatchQueue.main.asyncAfter(deadline: .now() + (final ? 0.05 : 0.6), execute: item)
        }

        private func showBoardActions(_ ids: [String]) {
            guard !ids.isEmpty else { return }
            let what = ids.count == 1 ? boardName(ids[0]) : "\(ids.count) boards"
            let pinned = ids.allSatisfy { pinnedBoards.contains($0) }
            let site = jiraSite
            var items: [(title: String, detail: String)] = []
            if ids.count == 1 { items.append(("Open board", "its issues here · pins it to the sidebar")) }
            items.append(pinned ? ("Unpin from sidebar", "\(what) · stops updating it")
                                : ("Pin to sidebar", "\(what) · polled like the other boards"))
            if !site.isEmpty { items.append(("Open in browser", what)) }
            w.showActionPicker(title: "Actions for \(what)", items: items) { [self] i in
                guard items.indices.contains(i) else { return }
                switch items[i].title {
                case "Open board": openListedBoard(ids[0])
                case "Pin to sidebar": setPinnedBoards(ids.filter { !pinnedBoards.contains($0) }, on: true)
                case "Unpin from sidebar": setPinnedBoards(ids, on: false)
                case "Open in browser":
                    for b in ids {
                        if let u = URL(string: "\(site)/secure/RapidBoard.jspa?rapidView=\(b)") { NSWorkspace.shared.open(u) }
                    }
                default: break
                }
            }
        }

        private func showActions() {
            let rows = w.actionRows.compactMap { $0 as? FieldRow }.filter { !$0.synthetic }
            guard !rows.isEmpty else { return }
            if onBoardList { showBoardActions(rows.compactMap { $0.fields["__board"] }); return }
            let n = rows.count, what = n == 1 ? (rows[0].fields["key"] ?? "1 row") : "\(n) rows"
            var items: [(title: String, detail: String)] = []
            let site = isJira ? jiraSite : ""
            let keyed = rows.filter { !($0.fields["key"] ?? "").isEmpty }
            let issues = keyed.filter { !jiraIsReleaseRow($0) }
            let releases = keyed.filter { jiraIsReleaseRow($0) }
            let urls = keyed.compactMap { r in jiraBrowseURL(r, site: site).map { (r, $0) } }
            let s = urls.count == 1 ? "" : "s"
            let noun = releases.isEmpty ? "issue" : issues.isEmpty ? "release" : "item"
            if !site.isEmpty && !urls.isEmpty {
                items.append(("Open in browser", "opens \(urls.count) \(noun)\(s) · the highlighted row + ticked rows"))
                items.append(("Copy URL and title", "\(urls.count) \(noun)\(s) · one “URL Title” line each"))
            }
            items.append(("Copy to clipboard", "\(what) · \(copyKeys.joined(separator: ", "))"))
            let tabFile = tabs.indices.contains(currentTab)
                ? (tabs[currentTab].path as NSString).lastPathComponent : ""
            if isJira && !issues.isEmpty {
                let pinned = issues.allSatisfy { favKeys.contains($0.fields["key"] ?? "") }
                let k = issues.count == 1 ? issues[0].fields["key"] ?? "" : "\(issues.count) issues"
                items.append(pinned
                    ? ("Remove from favorites", "unpin \(k) · \(JiraPoll.favoritesFile)")
                    : ("Add to favorites", "pin \(k) → \(JiraPoll.favoritesFile) · re-polled every run"))
                let labs = issueLabels(issues).filter { !pinnedLabels.contains($0) }
                if cmd.name == "jira", !labs.isEmpty {
                    items.append(("Pin labels to sidebar", labs.prefix(4).joined(separator: ", ")
                                  + (labs.count > 4 ? " +\(labs.count - 4)" : "")))
                }
            }
            if isJira && releases.count == 1 {
                items.append(("Show release issues", "every issue in \(releases[0].title) · one tab per release"))
            }
            if isJira && !releases.isEmpty {
                let starred = releases.allSatisfy { favReleases.contains($0.fields["key"] ?? "") }
                let k = releases.count == 1 ? releases[0].title : "\(releases.count) releases"
                items.append(starred
                    ? ("Unfavorite release", "remove \(k) from the sidebar")
                    : ("Favorite release", "pin \(k) in the sidebar · a click lists its issues"))
            }
            if isJira && !releases.isEmpty {
                let k = releases.count == 1 ? releases[0].title : "\(releases.count) releases"
                items.append(tabFile == JiraPoll.blacklistFile
                    ? ("Restore release", "show \(k) in the releases tab again")
                    : ("Blacklist release", "hide \(k) → \(JiraPoll.blacklistFile)"))
            }
            w.showActionPicker(title: "Actions for \(what)", items: items) { [self] i in
                guard items.indices.contains(i) else { return }
                switch items[i].title {
                case "Copy to clipboard":
                    let text = w.onCopyRows?(rows) ?? ""
                    host.copy(text, "\(n) row(s)")
                    w.showToast("Copied \(what)", symbol: "doc.on.clipboard")
                case "Copy URL and title":
                    let lines = urls.map { r, u -> String in
                        let t = (r.fields["title"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                        return u.absoluteString + (t.isEmpty ? "" : " \(t)")
                    }
                    host.copy(lines.joined(separator: "\n"), "\(urls.count) jira URL(s) + titles")
                    w.showToast("Copied \(urls.count) URL\(s) + title\(s)", symbol: "link")
                case "Open in browser":
                    let lines = urls.map { "\($0.0.fields["key"] ?? "")\t\($0.1.absoluteString)" }
                    for (_, u) in urls { NSWorkspace.shared.open(u) }
                    host.copy(lines.joined(separator: "\n"), "\(urls.count) jira key(s) + URLs")
                    w.showToast("Opened \(urls.count) · copied keys + URLs", symbol: "safari")
                    host.log("list '\(cmd.name)': opened \(urls.map { $0.1.absoluteString }.joined(separator: " "))")
                case "Add to favorites":
                    setFavorite(issues, on: true)
                case "Remove from favorites":
                    setFavorite(issues, on: false)
                case "Blacklist release":
                    setBlacklisted(releases, on: true)
                case "Restore release":
                    setBlacklisted(releases, on: false)
                case "Pin labels to sidebar":
                    setPinnedLabels(issueLabels(issues).filter { !pinnedLabels.contains($0) }, on: true)
                case "Favorite release":
                    setReleaseFavorite(releases, on: true)
                case "Unfavorite release":
                    setReleaseFavorite(releases, on: false)
                case "Show release issues":
                    host.showJiraReleaseView(releases[0])
                default:
                    break
                }
            }
        }

        private func showIconMenu() {
            let menu = NSMenu()
            menu.autoenablesItems = false
            host.addGlobalWindowItems(to: menu)
            menu.addItem(.separator())
            if cmd.name == "jira" {
                menu.addItem(menuItem("Search Jira…  ⌘F") { [host, weak w] in
                    guard let w else { return }
                    JiraSearchPanel.toggle(on: w, controller: host)
                })
                groupMenuItems().forEach(menu.addItem)
                menu.addItem(.separator())
                menu.addItem(menuItem("Import Favourite Filters") { [self] in importFilters() })
                menu.addItem(menuItem("Open Jira Config Window") { [host] in
                    host.showJiraDashboard()
                })
                if inSlot { menu.addItem(host.escHidesMenuItem(.jira)) }
            } else {
                if tabs.indices.contains(currentTab) {
                    let src = tabs[currentTab].path
                    menu.addItem(menuItem("Copy \(URL(fileURLWithPath: src).lastPathComponent) Path") { [host] in
                        host.copy(src, "source path: \(src)")
                    })
                }
                menu.addItem(menuItem("Copy Config Path") { [host] in
                    host.copy(settings.commandsConfPath, "config path: \(settings.commandsConfPath)")
                })
                menu.addItem(.separator())
                menu.addItem(host.openConfigMenuItem { [host] in host.openNoteFile($0) })
            }
            menu.addItem(.separator())
            host.addWindowSettingsItems(to: menu, window: w, section: configSection)
            if cmd.name == "jira" {
                menu.addItem(.separator())
                menu.addItem(menuItem("Disable Jira…") { [host] in
                    host.disableJiraAsking()
                })
            }
            menu.addItem(.separator())
            menu.addItem(host.shortcutsMenuItem(for: w, view: cmd.name == "jira" ? "jira" : ""))
            w.showHeaderMenu(menu)
        }

        private func watchTick() {
            refreshBadges()
            if isReleaseView, mtime(of: JiraPoll.issueCachePath) != cacheStamp {
                cacheStamp = mtime(of: JiraPoll.issueCachePath)
                JiraPoll.run("jira_poll.py", ["--release-view"])
            }
            if isJira, mtime(of: JiraPoll.boardCatalogPath) != catalogStamp {
                catalogStamp = mtime(of: JiraPoll.boardCatalogPath)
                catalog = JiraPoll.boardCatalog()
                syncReleasePins()
                updateBoardChrome()
            }
            if let p = pinView, p.key.hasPrefix(ListSession.boardPinPrefix) || p.key == ListSession.myWorkPrefix + "watching",
               mtime(of: p.path) != pinStamp {
                pinStamp = mtime(of: p.path)
                pinView?.items = host.loadListItems(p.path, cmd: cmd, columns: ListSession.tabColumns(cmd, p.path))
                invalidateFilter()
                w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            } else if let p = pinView, mtime(of: JiraPoll.issueCachePath) != cacheStamp {
                cacheStamp = mtime(of: JiraPoll.issueCachePath)
                let args: [String]
                switch pin(p.key) {
                case .label(let l): args = ["--label-view", l]
                case .myWork: args = ["--my-work"]
                case .release(let r): args = ["--release-view", r]
                case .board, .view, .boardList: args = []
                }
                if !args.isEmpty { JiraPoll.run("jira_poll.py", args) { [weak self] code, _, _ in
                    guard let self, code == 0, self.pinView?.key == p.key else { return }
                    self.pinView?.items = self.host.loadListItems(p.path, cmd: self.cmd,
                                                                  columns: ListSession.tabColumns(self.cmd, p.path))
                    self.invalidateFilter()
                    self.w.setRows(self.filteredRows(query: self.w.currentQuery), resetScroll: false)
                } }
            }
            var changed: [Int] = []
            for (i, t) in tabs.enumerated() {
                let mt = mtime(of: t.path)
                if mt != tabMtimes[i] {
                    tabMtimes[i] = mt
                    changed.append(i)
                }
            }
            guard !changed.isEmpty else { return }
            for i in changed {
                tabs[i].items = host.loadListItems(tabs[i].path, cmd: cmd, columns: ListSession.tabColumns(cmd, tabs[i].path))
                host.log("list '\(cmd.name)': reloaded \(tabs[i].path) after external write")
            }
            if isJira {
                favKeys = JiraPoll.favorites()
                favReleases = JiraPoll.favoriteReleases()
                pinnedLabels = JiraPoll.pinnedLabels()
                pinnedBoards = JiraPoll.pinnedBoards()
                pinnedViews = JiraPoll.pinnedViews()
                boards = JiraPoll.boardsInfo()
                syncReleasePins()
            }
            refreshBadges(force: true)
            w.tabTitles = tabs.map { URL(fileURLWithPath: $0.path).lastPathComponent }
            refreshPathLabel()
            guard pinView == nil, changed.contains(currentTab) else { return }
            applyFilterData()
            visibleOffset = 0
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            w.tabFooterText = ""
        }
    }
}

final class FontPanelReceiver: NSObject {
    static let shared = FontPanelReceiver()
    var onPick: ((String) -> Void)?
    @objc func changeFont(_ sender: Any?) {
        guard let fm = sender as? NSFontManager else { return }
        let f = fm.convert(NSFont.systemFont(ofSize: 13))
        if let fam = f.familyName { onPick?(fam) }
    }
}

// jira/paths.json: the file locations the poller (jira_paths.py) uses too,
// same env overrides
struct JiraPaths {
    let configJson, teamJson, legacyConfig, cacheDir, outDir: String
    let cache, tabs, sideDirs: [String: String]

    init(file: String, env: [String: String] = ProcessInfo.processInfo.environment) {
        let d = (try? Data(contentsOf: URL(fileURLWithPath: file)))
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        func path(_ envKey: String, _ key: String) -> String {
            if let e = env[envKey], !e.isEmpty { return e }
            return ((d[key] as? String ?? "") as NSString).expandingTildeInPath
        }
        configJson = path("JIRA_CONFIG_JSON", "configJson")
        teamJson = path("JIRA_TEAM_JSON", "teamJson")
        legacyConfig = path("JIRA_CONFIG_FILE", "legacyConfig")
        cacheDir = path("JIRA_CACHE_DIR", "cacheDir")
        outDir = ((d["outDir"] as? String ?? "") as NSString).expandingTildeInPath
        cache = d["cache"] as? [String: String] ?? [:]
        tabs = d["tabs"] as? [String: String] ?? [:]
        sideDirs = d["sideDirs"] as? [String: String] ?? [:]
        if d.isEmpty { wsLog("jira: \(file) missing or unreadable — jira paths unset") }
    }
    func cacheFile(_ name: String) -> String {
        (cacheDir as NSString).appendingPathComponent(cache[name] ?? name)
    }
}

enum JiraPoll {
    static var dir: String { assetDir + "/jira" }
    static let paths = JiraPaths(file: dir + "/paths.json")
    static var configPath: String { paths.configJson }
    static var statusPath: String { paths.cacheFile("status") }
    static var curlLogPath: String { paths.cacheFile("curlLog") }
    static var pollScript: String { dir + "/jira_poll.py" }
    static var lastEnableError: String?
    static var running: Set<String> = []
    static let intervals = ["5m", "10m", "15m", "30m", "1h", "2h", "4h", "1d", "1w"]
    static var directoryPath: String { paths.cacheFile("directory") }
    static var liveSearchFile: String { paths.tabs["liveSearch"] ?? "" }
    static var favoritesFile: String { paths.tabs["favorites"] ?? "" }
    static var issueCachePath: String { paths.cacheFile("issues") }
    static var releaseViewDir: String { paths.sideDirs["releases"] ?? "" }
    static var labelViewDir: String { paths.sideDirs["labels"] ?? "" }
    static var boardDir: String { paths.sideDirs["boards"] ?? "" }
    static var myWorkDir: String { paths.sideDirs["myWork"] ?? "" }
    static var boardsCachePath: String { paths.cacheFile("boards") }
    static var outDir: String {
        let o = readJSON(configPath)?["outDir"] as? String ?? ""
        return o.isEmpty ? paths.outDir : (o as NSString).expandingTildeInPath
    }
    static func sideDir(_ name: String) -> String {
        ((outDir as NSString).deletingLastPathComponent as NSString).appendingPathComponent(name)
    }
    static func pinnedBoards() -> [String] {
        readJSON(configPath)?["pinnedBoards"] as? [String] ?? []
    }
    struct BoardInfo {
        let name, type: String
        let columns: [(name: String, statuses: [String])]
        var quickFilters: [(id: String, name: String, jql: String)] = []
    }
    struct CatalogSprint { let id, name, state, start, end, complete: String }
    struct CatalogBoard { let id, name, type, project: String; let sprints: [CatalogSprint] }
    static var boardCatalogPath: String { paths.cacheFile("boardCatalog") }
    static func boardCatalog() -> [CatalogBoard] {
        var out: [CatalogBoard] = []
        for p in readJSON(boardCatalogPath)?["projects"] as? [[String: Any]] ?? [] {
            let proj = p["key"] as? String ?? ""
            for b in p["boards"] as? [[String: Any]] ?? [] {
                guard let id = b["id"] as? String, !out.contains(where: { $0.id == id }) else { continue }
                let sp = (b["sprints"] as? [[String: Any]] ?? []).compactMap { x -> CatalogSprint? in
                    guard let sid = x["id"] as? String else { return nil }
                    func f(_ k: String) -> String { x[k] as? String ?? "" }
                    return CatalogSprint(id: sid, name: f("name").isEmpty ? sid : f("name"), state: f("state"),
                                         start: f("startDate"), end: f("endDate"), complete: f("completeDate"))
                }
                out.append(CatalogBoard(id: id, name: b["name"] as? String ?? id, type: b["type"] as? String ?? "",
                                        project: proj, sprints: sp))
            }
        }
        return out
    }
    static func pinnedViews() -> [String] {
        readJSON(configPath)?["pinnedBoardViews"] as? [String] ?? []
    }
    static func boardsInfo() -> [String: BoardInfo] {
        var out: [String: BoardInfo] = [:]
        for (id, v) in readJSON(boardsCachePath) ?? [:] {
            guard let b = v as? [String: Any] else { continue }
            let cols = (b["columns"] as? [[String: Any]] ?? []).map {
                ($0["name"] as? String ?? "", $0["statuses"] as? [String] ?? [])
            }
            let qf = (b["quickFilters"] as? [[String: Any]] ?? []).compactMap { q -> (String, String, String)? in
                guard let qid = q["id"] as? String else { return nil }
                return (qid, q["name"] as? String ?? qid, q["jql"] as? String ?? "")
            }
            out[id] = BoardInfo(name: b["name"] as? String ?? id, type: b["type"] as? String ?? "", columns: cols,
                                quickFilters: qf)
        }
        return out
    }
    static var blacklistFile: String { paths.tabs["blacklistRelease"] ?? "" }

    static func favorites() -> Set<String> {
        Set(readJSON(configPath)?["favorites"] as? [String] ?? [])
    }
    static func favoriteReleases() -> [String] {
        readJSON(configPath)?["favoriteReleases"] as? [String] ?? []
    }
    static func pinnedLabels() -> [String] {
        readJSON(configPath)?["pinnedLabels"] as? [String] ?? []
    }

    static func windowSeconds(_ w: String?) -> TimeInterval? {
        guard let w, let unit = w.last, let n = Double(w.dropLast()) else { return nil }
        let mult: [Character: Double] = ["s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800]
        return mult[unit].map { n * $0 }
    }

    private static let stampParser: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return f
    }()
    static func parseStamp(_ s: String?) -> Date? {
        guard let s, !s.isEmpty else { return nil }
        return stampParser.date(from: s)
    }

    static func age(_ secs: TimeInterval) -> String {
        let s = max(0, Int(secs))
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        if s < 86400 { return "\(s / 3600)h" }
        return "\(s / 86400)d"
    }

    private static var titleCache: (stamp: Date?, names: [String: String]) = (nil, [:])
    static func listTitle(file: String) -> String? {
        let stamp = (try? FileManager.default.attributesOfItem(atPath: configPath))?[.modificationDate] as? Date
        if titleCache.stamp != stamp || titleCache.names.isEmpty {
            var names = [liveSearchFile: "Search results", blacklistFile: "Hidden releases"]
            for ep in endpoints {
                guard let name = ep["name"] as? String else { continue }
                let f = ep["file"] as? String ?? "\(name).json"
                if let t = ep["title"] as? String, !t.isEmpty { names[f] = t; continue }
                switch ep["type"] as? String {
                case "releases": names[f] = "Releases"
                case "favorites": names[f] = "Starred issues"
                default: names[f] = name == "all" ? "All projects" : name
                }
            }
            titleCache = (stamp, names)
        }
        return titleCache.names[file]
    }

    static func syncSummary(paths: [String], status: [String: Any]?, config: [String: Any]?) -> (text: String, tone: PopupTone)? {
        let eps = config?["endpoints"] as? [[String: Any]] ?? []
        var newest: Date?, behind = 0, polled = 0, failing = false
        for path in paths {
            let file = (path as NSString).lastPathComponent
            guard file != liveSearchFile, file != blacklistFile,
                  let ep = eps.first(where: { ($0["file"] as? String ?? "\($0["name"] as? String ?? "").json") == file }),
                  let name = ep["name"] as? String else { continue }
            polled += 1
            let entry = (status?["endpoints"] as? [[String: Any]] ?? []).first { ($0["name"] as? String) == name } ?? [:]
            if let ok = parseStamp(entry["lastSuccess"] as? String) { newest = max(newest ?? ok, ok) }
            let tone = tabBadge(path: path, status: status, config: config)?.tone
            if tone != .success { behind += 1 }
            if tone == .danger { failing = true }
        }
        guard polled > 0 else { return nil }
        guard let newest else { return ("Never synced", .danger) }
        let ago = age(Date().timeIntervalSince(newest))
        if behind > 0 { return ("\(behind) out of date · last sync \(ago) ago", failing ? .danger : .warning) }
        return ("Synced \(ago) ago", .success)
    }

    static func tabBadge(path: String, status: [String: Any]?, config: [String: Any]?) -> PopupTabBadge? {
        let file = (path as NSString).lastPathComponent
        let now = Date()
        if file == liveSearchFile {
            guard let m = (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
            else { return nil }
            return PopupTabBadge(tone: .dim, text: age(now.timeIntervalSince(m)),
                                 tip: "\(file): live search results from \(age(now.timeIntervalSince(m))) ago "
                                    + "(Cmd+F runs a new search) — not polled")
        }
        let eps = config?["endpoints"] as? [[String: Any]] ?? []
        let ep = eps.first { ($0["file"] as? String ?? "\($0["name"] as? String ?? "").json") == file }
            ?? (file == blacklistFile ? eps.first { ($0["type"] as? String) == "releases" } : nil)
        guard let ep, let name = ep["name"] as? String else { return nil }
        let entry = (status?["endpoints"] as? [[String: Any]] ?? []).first { ($0["name"] as? String) == name } ?? [:]
        let window = ep["window"] as? String ?? "10m"
        let wsec = windowSeconds(window) ?? 600
        let ok = parseStamp(entry["lastSuccess"] as? String)
        let failed = (entry["status"] as? String) == "error"
        let running = (entry["status"] as? String) == "running"
        let stale = ok.map { now.timeIntervalSince($0) > wsec + max(300, wsec / 2) } ?? true
        let tone: PopupTone = !stale && !failed ? .success : stale && failed ? .danger : .warning
        var tip = [ok.map { "\(file) · last polled \(age(now.timeIntervalSince($0))) ago (\(short(entry["lastSuccess"] as? String)))" }
                    ?? "\(file) · never polled successfully",
                   "job “\(name)” runs every \(window)"
                    + (stale ? " — out of date" : "") + (running ? " — polling now…" : "")]
        if failed, let e = entry["lastError"] as? String, !e.isEmpty {
            tip.append("last run failed (\(short(entry["lastRun"] as? String))): \(e)")
        }
        if file == blacklistFile { tip.append("releases hidden with Cmd+K ▸ Blacklist release") }
        switch status?["status"] as? String {
        case "setup pending"?: tip.append("scheduled polling is paused: setup pending (Jira Config ▸ Setup)")
        case "disabled"?: tip.append("polling is off: Jira is disabled")
        default: break
        }
        return PopupTabBadge(tone: tone, text: ok.map { age(now.timeIntervalSince($0)) } ?? "never",
                             tip: tip.joined(separator: "\n"))
    }

    static let baseFieldLabels: [String: String] = [
        "key": "Key", "title": "Title", "status": "Status", "assignee": "Assignee",
        "reporter": "Reporter", "priority": "Priority", "labels": "Labels",
        "description": "Description", "project": "Project", "updated": "Updated",
        "release": "Fix versions", "releaseLabel": "Release", "releaseDate": "Release date",
        "releaseStatus": "Released", "comments": "Comments",
        "components": "Components", "epic": "Epic / parent",
    ]

    static func fieldLabels() -> [String: String] {
        var out = baseFieldLabels
        let teamPath = (readJSON(configPath)?["teamConfig"] as? String).map { ($0 as NSString).expandingTildeInPath }
            ?? paths.teamJson
        guard let team = readJSON(teamPath) else { return out }
        func norm(_ k: String) -> String {
            k.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: #"[\s-]+"#, with: "_",
                                                                        options: .regularExpression).lowercased()
        }
        for (k, v) in team where norm(k) == "custom_fields" {
            for (alias, spec) in v as? [String: Any] ?? [:] {
                let d = (spec as? [String: Any] ?? [:]).reduce(into: [String: Any]()) { $0[norm($1.key)] = $1.value }
                out[alias] = (d["label"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? alias
            }
        }
        for (k, v) in team where norm(k) == "field_labels" {
            for (f, l) in v as? [String: String] ?? [:] where !l.trimmingCharacters(in: .whitespaces).isEmpty {
                out[f] = l.trimmingCharacters(in: .whitespaces)
            }
        }
        return out
    }

    static func labeled(_ cols: [ListColumn]) -> [ListColumn] {
        let labels = fieldLabels()
        return cols.map { c in
            var c = c
            if let l = labels[c.field] { c.title = l }
            return c
        }
    }

    /// One-shot asset-script runs go through the persistent worker now: same
    /// folder/PATH/no-bytecode contract as the old spawn, but no per-call
    /// process startup and no serialisation against other helper traffic.
    static func run(_ script: String, _ args: [String], stdin: String? = nil, folder: String? = nil,
                    done: ((Int32, String, String) -> Void)? = nil) {
        var params: [String: Any] = ["folder": folder ?? dir, "script": script, "args": args]
        if let stdin { params["stdin"] = stdin }
        pythonHelper.call("script.run", params, timeout: 1800) { result in
            switch result {
            case .success(let value):
                let d = value as? [String: Any]
                done?(Int32(d?["code"] as? Int ?? -1), d?["stdout"] as? String ?? "",
                      d?["stderr"] as? String ?? "")
            case .failure(let e):
                done?(-1, "", e.description)
            }
        }
    }

    /// Long jobs (setup, rebuild) keep spawning their own process: they run
    /// for minutes and are cancelled through `jira_poll.py --cancel`.
    static func spawn(_ script: String, _ args: [String], stdin: String? = nil, folder: String? = nil,
                      done: ((Int32, String, String) -> Void)? = nil) {
        DispatchQueue.global(qos: .userInitiated).async {
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:" + (env["PATH"] ?? "")
            let r: ProcessOutput
            do {
                r = try runProcess("/usr/bin/env", ["python3", (folder ?? dir) + "/" + script] + args,
                                   stdin: stdin ?? "", env: env)
            } catch {
                DispatchQueue.main.async { done?(-1, "", "cannot run python3: \(error)") }
                return
            }
            DispatchQueue.main.async { done?(r.code, r.out, r.err) }
        }
    }

    static func readJSON(_ path: String) -> [String: Any]? {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    static var status: [String: Any]? { readJSON(statusPath) }
    static var endpoints: [[String: Any]] { readJSON(configPath)?["endpoints"] as? [[String: Any]] ?? [] }

    static func owner(ofTab path: String) -> (kind: String, name: String, columns: [ListColumn])? {
        guard let d = readJSON(configPath) else { return nil }
        let file = (path as NSString).lastPathComponent
        if file == liveSearchFile {
            let ls = d["liveSearch"] as? [String: Any] ?? [:]
            return ("live", "search", ListColumn.parse(ls["columns"] as? String))
        }
        let parent = (path as NSString).deletingLastPathComponent
        if [releaseViewDir, labelViewDir, myWorkDir].contains(where: { parent.hasSuffix("/" + $0) }) {
            let plain = (d["endpoints"] as? [[String: Any]] ?? []).filter {
                ($0["type"] as? String ?? "issues") == "issues" && ($0["jql"] as? String ?? "").isEmpty }
            if let e = plain.first(where: { ($0["name"] as? String) == "all" }) ?? plain.first,
               let name = e["name"] as? String {
                return ("endpoint", name, ListColumn.parse(e["columns"] as? String))
            }
            return nil
        }
        for e in d["endpoints"] as? [[String: Any]] ?? [] {
            guard let name = e["name"] as? String, (e["type"] as? String) != "directory" else { continue }
            if (e["file"] as? String ?? "\(name).json") == file {
                return ("endpoint", name, ListColumn.parse(e["columns"] as? String))
            }
        }
        return nil
    }

    static func errorLine(_ err: String, fallback: String) -> String {
        let line = err.split(separator: "\n").map(String.init)
            .last(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) ?? fallback
        for p in ["jira-api: ", "jira-poll: ", "jira-config: ", "confluence-api: "] where line.hasPrefix(p) {
            return String(line.dropFirst(p.count))
        }
        return line
    }

    static func short(_ ts: String?) -> String {
        guard let ts, ts.count >= 16 else { return ts ?? "never" }
        let today = String(ISO8601DateFormatter.string(from: Date(), timeZone: .current,
                                                       formatOptions: [.withFullDate]))
        let hm = String(ts.dropFirst(11).prefix(5))
        return ts.hasPrefix(today) ? hm : String(ts.prefix(10)) + " " + hm
    }
}

extension SwitcherController {
    func toggleJiraPoll() {
        if jiraEnabledInConfig() {
            disableJiraAsking()
        } else {
            enableJiraChecked()
        }
    }

    func disableJiraAsking() {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Disable Jira?"
        var info = "The Jira poller is running (launchd, every 60s)"
        if !JiraPoll.running.isEmpty {
            info += " and a poll is in progress right now"
        }
        info += ". Keep polling in the background while Jira is disabled?\n\n"
            + "Keep Polling: the window and menu entries hide, the cache keeps updating.\n"
            + "Stop Polling: the launchd agent is unloaded — nothing touches the network."
        alert.informativeText = info
        alert.addButton(withTitle: "Stop Polling")
        alert.addButton(withTitle: "Keep Polling")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        switch alert.runModal() {
        case .alertFirstButtonReturn: setJiraEnabled(false, keepPolling: false)
        case .alertSecondButtonReturn: setJiraEnabled(false, keepPolling: true)
        default: log("jira: disable cancelled")
        }
    }

    func setJiraEnabled(_ on: Bool, keepPolling: Bool = false) {
        saveConfigValues(section: "jira", [
            ("enabled", on ? "true" : "false"),
            ("poll-when-disabled", !on && keepPolling ? "true" : nil),
        ])
        reloadConfig()
        log("jira: [jira] enabled = \(on)\(!on && keepPolling ? " (background polling kept)" : "") (menu-bar switch)")
        if on {
            JiraPoll.lastEnableError = nil
            JiraPoll.run("jira_status.py", ["--note-error"])
            JiraPoll.run("jira_config.py", ["--check"]) { [weak self] _, out, _ in
                let chk = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any]
                let setup = chk?["setup"] as? [String: Any]
                if setup?["state"] as? String == "pending" { self?.showJiraDashboard() }
                else { self?.showCommand("jira") }
            }
        } else if let w = subWindows.first(where: { $0.config.name == "jira" }) {
            w.hide(restore: false)
        }
    }

    func enableJiraChecked() {
        JiraPoll.run("jira_config.py", ["--check"]) { [weak self] _, out, err in
            guard let self else { return }
            let chk = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any]
            let problems = chk?["problems"] as? [String] ?? [JiraPoll.errorLine(err, fallback: "config unreadable")]
            if chk?["exists"] as? Bool != true || !problems.isEmpty {
                self.log("jira: enable -> setup window (\(problems.joined(separator: "; ")))")
                self.showJiraSetup(reason: problems.isEmpty
                    ? "No Jira config yet — fill in your site and API token."
                    : problems.joined(separator: "\n"))
                return
            }
            if (chk?["projectKeys"] as? [String] ?? []).isEmpty {
                self.log("jira: enable -> setup window (no projects in scope)")
                self.showJiraSetup(reason: "Enter the projects in scope — every Jira query is limited to them.")
                return
            }
            JiraPoll.run("jira_api.py", ["--myself"]) { [weak self] code, _, err in
                guard let self else { return }
                if code == 0 {
                    self.setJiraEnabled(true)
                    return
                }
                let msg = JiraPoll.errorLine(err, fallback: "login test failed (exit \(code))")
                JiraPoll.lastEnableError = msg
                JiraPoll.run("jira_status.py", ["--note-error", "login failed: " + msg])
                self.log("jira: enable refused — \(msg)")
                let alert = NSAlert()
                alert.alertStyle = .warning
                alert.messageText = "Jira login failed — polling stays off"
                alert.informativeText = msg + "\n\nEvery request is logged (copy-pasteable) in "
                    + JiraPoll.curlLogPath
                alert.addButton(withTitle: "Setup…")
                alert.addButton(withTitle: "Close")
                NSApp.activate(ignoringOtherApps: true)
                if alert.runModal() == .alertFirstButtonReturn {
                    self.showJiraSetup(reason: msg)
                }
            }
        }
    }

    func showJiraSetup(reason: String? = nil) {
        JiraSetupWindow.show(controller: self, reason: reason)
    }

    func showConfluence(setup: Bool = false) {
        guard confluenceEnabled() else { return }
        if settings.sharedWindow {
            if !slot.isVisible { (savedWID, savedPID) = readFocusFile() }
            slot.open(.confluence)
        } else {
            ConfluenceWindow.create(controller: self, frame: nil)
            ConfluenceWindow.current?.showStandalone()
        }
        if setup { ConfluenceWindow.current?.showSetup() }
    }

    func showCompare(_ paths: [String] = [], titles: [CompareSide: String] = [:], git: Bool = false,
                     waiter: (() -> Void)? = nil) {
        guard compareEnabled() else { waiter?(); return }
        if settings.sharedWindow {
            if !slot.isVisible { (savedWID, savedPID) = readFocusFile() }
            slot.open(.compare)
        } else {
            CompareWindow.create(controller: self, frame: nil)
            CompareWindow.current?.showStandalone()
        }
        guard !paths.isEmpty, let w = CompareWindow.current else { if !paths.isEmpty { waiter?() }; return }
        w.openPair(paths[0], paths.count > 1 ? paths[1] : nil, titles: titles, git: git, waiter: waiter)
    }

    func handleCompareMessage(_ words: [String], reply: (() -> Void)?) {
        var paths: [String] = []
        var titles: [CompareSide: String] = [:]
        var i = 0
        while i < words.count {
            switch words[i] {
            case "--wait": break
            case "--title1" where i + 1 < words.count: titles[.left] = words[i + 1]; i += 1
            case "--title2" where i + 1 < words.count: titles[.right] = words[i + 1]; i += 1
            default: paths.append(words[i])
            }
            i += 1
        }
        showCompare(paths, titles: titles, git: reply != nil, waiter: reply)
    }

    func compareTestDo(_ a: String) -> String? {
        let parts = a.split(separator: ":", maxSplits: 1).map(String.init)
        switch parts.first ?? "" {
        case "open", "open-sub":
            let ps = (parts.count > 1 ? parts[1] : "").split(separator: "|", omittingEmptySubsequences: false)
                .map { String($0).trimmingCharacters(in: .whitespaces) }
            let l = ps.first.flatMap { $0.isEmpty ? nil : $0 }, r = ps.count > 1 && !ps[1].isEmpty ? ps[1] : nil
            if parts[0] == "open" {
                showCompare()
                CompareWindow.current?.openPair(l, r)
            } else {
                showCompare()
                let sub = CompareWindow.createSub(controller: self, frame: slot.currentFrame())
                sub.openPair(l, r)
                slot.push(.compareText)
            }
            return nil
        case "back":
            slot.back(esc: true)
            return nil
        default:
            let w = slot.current == .compareText ? CompareWindow.sub : CompareWindow.current
            guard let w else { return "the compare view isn't open" }
            return w.testDo(a)
        }
    }

    func showAI() {
        guard aiEnabled() else { return }
        if settings.sharedWindow {
            if !slot.isVisible { (savedWID, savedPID) = readFocusFile() }
            slot.open(.ai)
        } else {
            AIWindow.create(controller: self, frame: nil)
            AIWindow.current?.showStandalone()
        }
    }

    func showJiraDashboard() {
        guard settings.sharedWindow else {
            JiraDashboardWindow.show(controller: self)
            return
        }
        if !slot.isVisible { (savedWID, savedPID) = readFocusFile() }
        JiraDashboardWindow.show(controller: self, present: false)
        JiraDashboardWindow.current?.onSlotBack = { [weak self] in self?.slot.back() }
        slot.push(.config)
    }

    func jiraPollNow(_ endpoint: String, full: Bool = false, done: (() -> Void)? = nil) {
        guard !JiraPoll.running.contains(endpoint) else { return }
        JiraPoll.running.insert(endpoint)
        log("jira: poll now (\(endpoint)\(full ? ", full resync" : ""))")
        let args = ["--force", "--quiet", "--projects", endpoint] + (full ? ["--init"] : [])
        JiraPoll.run("jira_poll.py", args) { [weak self] code, _, err in
            JiraPoll.running.remove(endpoint)
            self?.log("jira: poll \(endpoint) finished (exit \(code))"
                      + (code == 0 ? "" : ": " + JiraPoll.errorLine(err, fallback: "see status.json")))
            done?()
        }
    }

    func reloadJiraWindow() {
        guard let w = subWindows.first(where: { $0.config.name == "jira" }) else { return }
        if settings.sharedWindow {
            let wasCurrent = slot.current == .jira && w.isShown
            let f = w.nativeWindow.frame
            jiraShowTab = nil
            closeSlotWindow(w)
            subWindows.removeAll { $0 === w }
            if wasCurrent, ensureSlotMember(.jira, frame: f) { slot.present(.jira) }
            return
        }
        let wasShown = w.isShown
        jiraShowTab = nil
        w.hide(restore: false)
        subWindows.removeAll { $0 === w }
        w.releaseHooks()
        w.nativeWindow.orderOut(nil)
        if wasShown { showCommand("jira") }
    }
}

final class JiraSetupWindow: NSObject, NSWindowDelegate {
    private static var live: JiraSetupWindow?

    private let window: NSWindow
    private let site = NSTextField()
    private let email = NSTextField()
    private let token = NSSecureTextField()
    private let project = NSTextField()
    private let maxResults = NSTextField()
    private let notice = NSTextField(wrappingLabelWithString: "")
    private let result = NSTextField(wrappingLabelWithString: "")
    private let testButton = NSButton(title: "Test Connection", target: nil, action: nil)
    private let saveButton = NSButton(title: "Save & Enable", target: nil, action: nil)
    private let curlButton = NSButton(title: "Copy curl", target: nil, action: nil)
    private var monitor: Any?
    private weak var controller: SwitcherController?
    private var detectedAuth: String?

    static func show(controller: SwitcherController, reason: String?) {
        if let w = live {
            if let reason { w.setResult(reason, ok: nil) }
            NSApp.activate(ignoringOtherApps: true)
            w.window.makeKeyAndOrderFront(nil)
            return
        }
        let w = JiraSetupWindow(controller: controller)
        live = w
        if let reason { w.setResult(reason, ok: nil) }
        w.prefill()
        NSApp.activate(ignoringOtherApps: true)
        w.window.center()
        w.window.makeKeyAndOrderFront(nil)
        w.window.makeFirstResponder(w.site)
    }

    private init(controller: SwitcherController) {
        self.controller = controller
        let W: CGFloat = 520, H: CGFloat = 372
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: W, height: H),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        window.title = "Jira Setup"
        window.isReleasedWhenClosed = false
        window.level = NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 1)
        window.delegate = self
        let content = NSView(frame: NSRect(x: 0, y: 0, width: W, height: H))
        let head = NSTextField(wrappingLabelWithString:
            "Paste your token — the auth type is detected on Test / Save. Server / Data Center "
            + "personal access tokens go out as Authorization: Bearer; a Jira Cloud API token also "
            + "needs the account email.")
        head.font = .systemFont(ofSize: 12)
        head.frame = NSRect(x: 20, y: H - 50, width: W - 40, height: 34)
        content.addSubview(head)
        let rows: [(String, NSTextField, String)] = [
            ("Site URL", site, "https://jira.example.com"),
            ("Email (Cloud only)", email, "only for *.atlassian.net — blank for a personal access token"),
            ("Token", token, "personal access token / API token"),
            ("Projects in scope", project, "e.g. SAM1, KAN — required; every query stays inside these"),
            ("Max results", maxResults, "25"),
        ]
        var y = H - 88
        for (label, field, placeholder) in rows {
            let l = NSTextField(labelWithString: label)
            l.alignment = .right
            l.frame = NSRect(x: 20, y: y + 3, width: 110, height: 18)
            field.frame = NSRect(x: 140, y: y, width: W - 160, height: 24)
            field.placeholderString = placeholder
            field.usesSingleLineMode = true
            field.cell?.wraps = false
            field.cell?.isScrollable = true
            content.addSubview(l)
            content.addSubview(field)
            y -= 34
        }
        notice.font = .systemFont(ofSize: 11)
        notice.textColor = .secondaryLabelColor
        notice.frame = NSRect(x: 20, y: y - 18, width: W - 40, height: 40)
        content.addSubview(notice)
        result.font = .systemFont(ofSize: 12)
        result.frame = NSRect(x: 20, y: 58, width: W - 40, height: 44)
        content.addSubview(result)
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel(_:)))
        cancel.keyEquivalent = "\u{1b}"
        for b in [testButton, saveButton, cancel, curlButton] { b.bezelStyle = .rounded }
        curlButton.target = self
        curlButton.action = #selector(copyCurl(_:))
        curlButton.toolTip = "Copy the login test (GET /rest/api/2/myself) as a curl command"
        saveButton.keyEquivalent = "\r"
        saveButton.target = self
        saveButton.action = #selector(saveAndEnable(_:))
        testButton.target = self
        testButton.action = #selector(test(_:))
        saveButton.frame = NSRect(x: W - 20 - 130, y: 16, width: 130, height: 30)
        cancel.frame = NSRect(x: saveButton.frame.minX - 96, y: 16, width: 90, height: 30)
        testButton.frame = NSRect(x: 20, y: 16, width: 140, height: 30)
        curlButton.frame = NSRect(x: testButton.frame.maxX + 6, y: 16, width: 100, height: 30)
        content.addSubview(testButton)
        content.addSubview(curlButton)
        content.addSubview(cancel)
        content.addSubview(saveButton)
        window.contentView = content
        window.autorecalculatesKeyViewLoop = true
        window.initialFirstResponder = site
        installEditShortcuts()
    }

    private func installEditShortcuts() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.window.isKeyWindow else { return e }
            if e.keyCode == 53 { self.close(); return nil }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command), ctrl = mods.contains(.control)
            guard cmd || ctrl, let ed = self.window.firstResponder as? NSText else { return e }
            switch e.keyCode {
            case 9: ed.paste(nil)
            case 8: ed.copy(nil)
            case 0 where cmd: ed.selectAll(nil)
            case 7 where cmd: ed.cut(nil)
            case 6 where cmd: ed.undoManager?.undo()
            default: return e
            }
            return nil
        }
    }

    private func prefill() {
        JiraPoll.run("jira_config.py", ["--check"]) { [weak self] _, out, _ in
            guard let self,
                  let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any]
            else { return }
            if self.site.stringValue.isEmpty { self.site.stringValue = d["site"] as? String ?? "" }
            if self.email.stringValue.isEmpty, d["auth"] as? String != "bearer" {
                self.email.stringValue = d["email"] as? String ?? ""
            }
            if self.project.stringValue.isEmpty {
                let keys = d["projectKeys"] as? [String] ?? []
                self.project.stringValue = keys.isEmpty ? (d["defaultProject"] as? String ?? "")
                    : keys.joined(separator: ", ")
            }
            if self.maxResults.stringValue.isEmpty, let m = d["defaultMax"] as? Int {
                self.maxResults.stringValue = String(m)
            }
            let notes = d["notes"] as? [String] ?? []
            var lines = ["Saves to \(JiraPoll.configPath) (chmod 600)."]
            for k in ["SITE", "EMAIL", "TOKEN"] where notes.contains(where: { $0.contains("env JIRA_\(k)") }) {
                lines.append("Using JIRA_\(k) from the environment — save will persist it into config.json.")
            }
            if d["hasToken"] as? Bool == true {
                self.token.placeholderString = "token set — leave blank to keep it"
            }
            self.notice.stringValue = lines.joined(separator: "\n")
        }
    }

    private func setResult(_ text: String, ok: Bool?) {
        result.stringValue = text
        result.textColor = ok == true ? .systemGreen : ok == false ? .systemRed : .secondaryLabelColor
    }

    private func busy(_ on: Bool) {
        testButton.isEnabled = !on
        saveButton.isEnabled = !on
        curlButton.isEnabled = !on
    }

    private func typedArgs() -> (args: [String], stdin: String?) {
        let v = trimmed
        var args: [String] = ["--email", v.email], stdin: String? = nil
        if !v.site.isEmpty { args += ["--site", v.site] }
        if !v.token.isEmpty { args.append("--token-stdin"); stdin = v.token + "\n" }
        return (args, stdin)
    }

    private static func authTitle(_ mode: String) -> String {
        mode == "basic" ? "email + API token (Cloud)" : "Bearer token (Server / Data Center)"
    }

    private func detect(done: @escaping (String?, String) -> Void) {
        let t = typedArgs()
        JiraPoll.run("jira_api.py", ["--detect-auth"] + t.args, stdin: t.stdin) { [weak self] code, out, err in
            let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            if code == 0, let auth = d["auth"] as? String {
                self?.detectedAuth = auth
                done(auth, d["user"] as? String ?? "unknown user")
                return
            }
            let tried = (d["tried"] as? [[String: Any]] ?? []).map {
                "\($0["mode"] as? String ?? "?") → HTTP \(($0["http"] as? Int).map(String.init) ?? "?")"
            }.joined(separator: ", ")
            let msg = d["error"] as? String ?? JiraPoll.errorLine(err, fallback: "login failed (exit \(code))")
            done(nil, msg + (tried.isEmpty ? "" : " (tried \(tried))") + " — Copy curl to reproduce in a terminal")
        }
    }

    @objc private func copyCurl(_ sender: Any?) {
        let t = typedArgs()
        let auth = detectedAuth ?? (trimmed.email.isEmpty ? "bearer" : "basic")
        JiraPoll.run("jira_api.py", ["--curl", "--myself", "--auth", auth] + t.args, stdin: t.stdin) { [weak self] code, out, err in
            let cmd = out.trimmingCharacters(in: .whitespacesAndNewlines)
            guard code == 0, !cmd.isEmpty else {
                self?.setResult("✗ \(JiraPoll.errorLine(err, fallback: "could not build curl (exit \(code))"))", ok: false)
                return
            }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(cmd, forType: .string)
            self?.setResult("curl copied (includes the token) — paste into a terminal to test the login.", ok: nil)
        }
    }

    private var trimmed: (site: String, email: String, token: String, project: String, max: String) {
        let t = { (f: NSTextField) in f.stringValue.trimmingCharacters(in: .whitespacesAndNewlines) }
        return (t(site), t(email), t(token), t(project), t(maxResults))
    }

    static func parseProjectKeys(_ raw: String) -> (keys: [String], bad: [String]) {
        var keys: [String] = [], bad: [String] = []
        for part in raw.uppercased().split(whereSeparator: { $0 == "," || $0.isWhitespace }) {
            let k = String(part)
            if k.range(of: "^[A-Z][A-Z0-9_]*$", options: .regularExpression) == nil { bad.append(k) }
            else if !keys.contains(k) { keys.append(k) }
        }
        return (keys, bad)
    }

    @objc private func test(_ sender: Any?) {
        busy(true)
        setResult("Testing (detecting the auth type)…", ok: nil)
        detect { [weak self] auth, msg in
            self?.busy(false)
            guard let auth else { self?.setResult("✗ \(msg)", ok: false); return }
            self?.setResult("✓ Connected as \(msg) — \(Self.authTitle(auth))", ok: true)
        }
    }

    @objc private func saveAndEnable(_ sender: Any?) {
        let v = trimmed
        guard !v.site.isEmpty, v.site.hasPrefix("http") else {
            setResult("✗ Site URL must start with https://", ok: false)
            return
        }
        let scope = Self.parseProjectKeys(v.project)
        guard scope.bad.isEmpty else {
            setResult("✗ Not a project key: \(scope.bad.joined(separator: ", ")) — use keys like SAM1, KAN", ok: false)
            window.makeFirstResponder(project)
            return
        }
        guard !scope.keys.isEmpty else {
            setResult("✗ Enter at least one project in scope — every query is limited to these projects", ok: false)
            window.makeFirstResponder(project)
            return
        }
        busy(true)
        setResult("Detecting the auth type…", ok: nil)
        detect { [weak self] auth, msg in
            guard let self else { return }
            let mode = auth ?? self.detectedAuth ?? (v.email.isEmpty ? "bearer" : "basic")
            var obj: [String: Any] = ["site": v.site, "email": mode == "basic" ? v.email : "",
                                      "auth": mode,
                                      "defaultProject": scope.keys[0], "defaultMax": Int(v.max) ?? 25]
            if !v.token.isEmpty { obj["token"] = v.token }
            guard let data = try? JSONSerialization.data(withJSONObject: obj) else { self.busy(false); return }
            self.setResult("Saving…", ok: nil)
            JiraPoll.run("jira_config.py", ["--save"], stdin: String(decoding: data, as: UTF8.self)) {
                [weak self] code, _, err in
                guard let self else { return }
                guard code == 0 else {
                    self.busy(false)
                    self.setResult("✗ save failed: \(JiraPoll.errorLine(err, fallback: "exit \(code)"))", ok: false)
                    return
                }
                self.saveScope(scope.keys) { ok in
                    if ok { self.finishSave(auth: auth, mode: mode, msg: msg) }
                }
            }
        }
    }

    private func saveScope(_ keys: [String], done: @escaping (Bool) -> Void) {
        let json = String(decoding: (try? JSONSerialization.data(withJSONObject: keys)) ?? Data("[]".utf8),
                          as: UTF8.self)
        JiraPoll.run("jira_config.py", ["--team-set", "project_keys"], stdin: json) { [weak self] code, out, err in
            guard let self else { return }
            let r = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any] ?? [:]
            if code == 0, r["ok"] as? Bool == true { done(true); return }
            self.busy(false)
            let probs = (r["problems"] as? [String] ?? []).joined(separator: "; ")
            self.setResult("✗ projects in scope not saved: "
                           + (probs.isEmpty ? JiraPoll.errorLine(err, fallback: "exit \(code)") : probs), ok: false)
            done(false)
        }
    }

    private func finishSave(auth: String?, mode: String, msg: String) {
        busy(false)
        guard auth != nil else {
            setResult("✗ saved, but login failed: \(msg) — polling stays off", ok: false)
            JiraPoll.lastEnableError = msg
            return
        }
        setResult("✓ Connected as \(msg) — \(Self.authTitle(mode)) — enabling…", ok: true)
        let c = controller
        close()
        c?.setJiraEnabled(true)
    }

    @objc private func cancel(_ sender: Any?) { close() }

    private func close() {
        window.orderOut(nil)
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        JiraSetupWindow.live = nil
    }

    func windowWillClose(_ notification: Notification) {
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        JiraSetupWindow.live = nil
    }
}

final class FileFastPasteView: NSTextView {
    weak var hint: NSTextField?
    var placeholder = "paste output"
    var placeholderColor = NSColor.secondaryLabelColor
    var pastedColor = NSColor.systemGreen
    override func didChangeText() {
        super.didChangeText()
        refreshIndicator()
    }
    func refreshIndicator() {
        guard let hint else { return }
        if string.isEmpty {
            hint.stringValue = placeholder
            hint.textColor = placeholderColor
        } else {
            let lines = string.split(separator: "\n", omittingEmptySubsequences: false).count
            hint.stringValue = "************************  \(lines) line\(lines == 1 ? "" : "s"), \(string.count) chars pasted"
            hint.textColor = pastedColor
        }
        hint.isHidden = false
    }
    override func insertNewline(_ sender: Any?) { insertNewlineIgnoringFieldEditor(sender) }
}
