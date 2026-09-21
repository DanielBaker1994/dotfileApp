import AppKit
import Foundation
import Darwin
import AVFoundation
import Speech

// ============================================================================
// Workspace switcher — host app built on the PopupWindow framework.
// This file only contains app-specific logic: workspace/command data, the
// aerospace socket IPC, app icons, the persistent shell, and behavior hooks.
// ============================================================================

// MARK: - Paths & identities

// Derived from the binary location; everything else environment-specific
// lives in `app` (AppSettings) below and is overridable from commands.conf.
let binDir: String = {
    let u = URL(fileURLWithPath: CommandLine.arguments[0]).absoluteURL
    let p = u.path
    // .app bundle: the binary lives at <dir>/<name>.app/Contents/MacOS/<bin>,
    // but the app's assets (commands.conf, icons) sit NEXT TO the bundle.
    // TCC grants key by the bundle id (stable across rebuilds) — the bundle
    // is what keeps mic/speech permissions working.
    if let r = p.range(of: "/Contents/MacOS/") {
        var d = String(p[..<r.lowerBound])
        if d.hasSuffix(".app") { d = (d as NSString).deletingLastPathComponent }
        return d
    }
    return u.deletingLastPathComponent().path
}()

// The config file name is a constant (it must be findable before any config
// is read); every OTHER string — shell, paths, sockets, icon names — is
// configurable via the [app] section of this file.
let commandsConfName = "commands.conf"

// MARK: - App settings (commands.conf [app] section)

// Single source of truth for every machine-owned string: shell, CLI paths,
// socket/focus filenames, icon assets, search dirs, and launchd service wiring.
// Defaults live here so the app works with no config; parseAppConfig() applies
// the [app] section overrides when commands.conf is loaded at startup.
struct AppSettings {
    var shell = "/opt/homebrew/bin/bash"
    // args for the embedded terminal's shell: --login -i sources the profile
    // AND rc files so aliases/functions (zoxide, etc.) work there
    var shellArgs: [String] = ["--login", "-i"]
    var terminalFont = "Hack Nerd Font"
    var aerospaceCLI = ["/opt/homebrew/bin/aerospace",
                        "/usr/local/bin/aerospace", "aerospace"]
    var colorSources = [NSString(string: "~/.config/sketchybar/colors.sh").expandingTildeInPath,
                        NSString(string: "~/.config/sketchybar/plugins/aerospacer.sh").expandingTildeInPath]
    var appDirs = ["/Applications", "/Applications/Utilities",
                   "/System/Applications", "/System/Applications/Utilities",
                   "/System/Library/CoreServices",
                   NSHomeDirectory() + "/Applications"]
    var jiraIconName = "jira_icon.png"
    var notesIconName = "notes_icon.png"
    var notesSocketName = "ws-notes.sock"
    var focusFileName = "workspace-switcher-focus"
    var focusBridgeName = "ws-aerospace-focus"
    var switcherWindowName = "workspace-switcher"
    var detailWindowName = "jira-detail"
    var aerospaceSocketPath = "/tmp/bobko.aerospace-\(NSUserName()).sock"
    var crashLogPath = NSString(string: "~/.cache/ws-crash.log").expandingTildeInPath
    var aeroDebugFlag = NSString(string: "~/.cache/aero-debug").expandingTildeInPath
    var aeroLog = NSString(string: "~/.cache/ws-aero.log").expandingTildeInPath
    var authDebugFlag = NSString(string: "~/.cache/ws-auth-debug").expandingTildeInPath
    var voiceLocale = "en-US"
    // derived (recomputed whenever the settings change)
    var commandsConfPath: String { binDir + "/" + commandsConfName }
    var focusFilePath: String { popupTmpDir() + focusFileName }
    var jiraIconPath: String { binDir + "/" + jiraIconName }
    var notesIconPath: String { binDir + "/" + notesIconName }
}
var settings = AppSettings()

// MARK: - Tunables (named constants for the numeric magic)

let ipcSocketTimeout = 1.0    // s: aerospace socket reads + launcher ping
let ipcFallbackTimeout = 1.5  // s: aerospace CLI fallback kill timeout
let serverRecvTimeout = 2.0   // s: command-server socket recv timeout
let focusPollInterval = 0.25  // s: focus-bridge file poller
let noteWatchInterval = 1.0   // s: note external-write watcher
let listWatchInterval = 1.5   // s: list reload watcher
let defaultNoteSize = CGSize(width: 640, height: 440)
let defaultListSize = CGSize(width: 520, height: 520)
let defaultOutputSize = CGSize(width: 780, height: 560)
let defaultDetailSize = CGSize(width: 820, height: 640)

// MARK: - Crash reporter

// Writes a backtrace to ~/.cache/ws-crash.log on any fatal signal, then
// re-raises the signal so the OS still records its normal crash report.
// Install at launch with installCrashHandler() — the log pinpoints exactly
// where a crash happened (frame addresses + symbol names).
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
    // restore the default disposition and re-raise so the system generates
    // its own crash report alongside our backtrace
    signal(sig, SIG_DFL)
    raise(sig)
}

func installCrashHandler() {
    for sig in [SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGSEGV, SIGFPE] {
        signal(sig, crashHandler)
    }
}

// colors: parsed from the sketchybar scripts in settings.colorSources (searched
// in order; configured via the [app] color-sources key)
func parseColors() -> [String: NSColor] {
    var out: [String: NSColor] = [:]
    for path in settings.colorSources {
        guard let content = try? String(contentsOfFile: path, encoding: .utf8) else { continue }
        let ns = content as NSString
        let regex = try! NSRegularExpression(
            pattern: "^([A-Z_]+)=0x([0-9a-fA-F]{8})",
            options: [.anchorsMatchLines])
        for m in regex.matches(in: content, range: NSRange(location: 0, length: ns.length)) {
            let key = ns.substring(with: m.range(at: 1))
            let hex = ns.substring(with: m.range(at: 2))
            var v: UInt64 = 0
            Scanner(string: hex).scanHexInt64(&v)
            // 0xAARRGGBB — alpha ignored (matches the Python build's opaque look)
            let r = Double((v >> 16) & 0xFF) / 255.0
            let g = Double((v >> 8) & 0xFF) / 255.0
            let b = Double(v & 0xFF) / 255.0
            out[key] = NSColor(srgbRed: r, green: g, blue: b, alpha: 1)
        }
    }
    return out
}

// [theme] section in commands.conf: friendly hex colors that override the
// sketchybar-derived window colors app-wide. Keys map 1:1 to the popup's
// color roles (background border text dim highlight accent header panel).
// The interactive color picker edits this section live.
func parseTheme() -> [String: NSColor] {
    var out: [String: NSColor] = [:]
    guard let content = try? String(contentsOfFile: settings.commandsConfPath,
                                    encoding: .utf8) else { return out }
    var inTheme = false
    for line in content.split(separator: "\n") {
        let s = line.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("[") && s.hasSuffix("]") {
            inTheme = s == "[theme]"
            continue
        }
        guard inTheme, let eq = s.firstIndex(of: "=") else { continue }
        let key = s[..<eq].trimmingCharacters(in: .whitespaces).lowercased()
        let val = s[s.index(after: eq)...].trimmingCharacters(in: .whitespaces)
        if let c = hexColor(val) { out[key] = c }
    }
    return out
}

let C = parseColors()
let THEME = parseTheme()
let BAR = THEME["background"] ?? C["BAR_COLOR"] ?? NSColor.black
let GROUP_BG = THEME["highlight"] ?? C["GROUP_BG_COLOR"] ?? NSColor.gray
let TEXT = THEME["text"] ?? C["WHITE"] ?? NSColor.white
let DIM = THEME["dim"] ?? C["GREY"] ?? NSColor.gray
let BORDER = THEME["border"] ?? C["SPACE_BORDER_COLOR"] ?? NSColor.white
let ACCENT = THEME["accent"] ?? NSColor(srgbRed: 85/255, green: 104/255, blue: 130/255, alpha: 1)
// app-wide default drag-header tint + the two drawer backgrounds ([theme]
// header / browser / terminal); per-window `header-color` /
// `browser-background` / `terminal-background` in commands.conf override them
let THEME_HEADER = THEME["header"]
let THEME_BROWSER = THEME["browser"]
let THEME_TERMINAL = THEME["terminal"]

// default drag-header tint for the notes/jira windows (dark bluey silver);
// a commands.conf `header-color` or [theme] `header` overrides it per scope
let headerBlueSilver = THEME_HEADER ?? NSColor(srgbRed: 0.27, green: 0.31, blue: 0.36, alpha: 1)

// MARK: - Focus file (captured by the launcher at keypress time)

// Ask a RUNNING daemon to open the notes window directly (isolated notes
// launch). Returns false when no daemon is listening.
@discardableResult
func sendLaunchMessage(_ name: String) -> Bool {
    let path = popupTmpDir() + settings.notesSocketName
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    // non-blocking connect + 1s select: a daemon whose accept queue is
    // saturated would otherwise block this ping (and the hotkey script
    // behind it) for minutes
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
    _ = fcntl(fd, F_SETFL, flags)   // blocking write, tiny payload
    let msg = name + "\n"
    msg.withCString { _ = write(fd, $0, msg.count) }
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

// MARK: - Aerospace IPC (direct socket; falls back to spawning the CLI)

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

func aerospaceSocket(_ args: [String]) -> String? {
    let path = settings.aerospaceSocketPath
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
    // aerospace IPC can stall (its own main thread is busy) — never let a
    // blocking read hang OUR main thread: time out and fall back
    var tv = timeval(tv_sec: Int(ipcSocketTimeout), tv_usec: 0)
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
        // don't wait forever on a stalled CLI — terminate after the timeout
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
    // TEMP DEBUG: time every IPC call, logged while ~/.cache/aero-debug exists
    let t0 = DispatchTime.now().rawValue
    let r = aerospaceSocket(args) ?? aerospaceFallback(args)
    let ms = Double(DispatchTime.now().rawValue - t0) / 1_000_000
    if FileManager.default.fileExists(atPath: settings.aeroDebugFlag) {
        let line = String(format: "%.1fms %@ -> [%@]\n", ms, args.joined(separator: " "), r)
        if let fh = FileHandle(forWritingAtPath: settings.aeroLog) {
            fh.seekToEndOfFile()
            fh.write(Data(line.utf8))
            try? fh.close()
        } else {
            FileManager.default.createFile(atPath: settings.aeroLog, contents: Data(line.utf8))
        }
    }
    return r
}

// MARK: - Data model

struct AppInfo {
    let name: String
    let bundleID: String?
    let windowTitle: String?
}

struct WorkspaceInfo {
    let id: String
    var apps: [AppInfo]
}

func gatherWorkspaces() -> [WorkspaceInfo] {
    let order = aerospaceCall(["list-workspaces", "--all"])
        .split(separator: "\n").map(String.init)
    var dict = Dictionary(uniqueKeysWithValues: order.map { ($0, WorkspaceInfo(id: $0, apps: [])) })
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
    // letters (alphabetical) first, then numbers (numeric)
    return order.compactMap { dict[$0] }.sorted { a, b in
        let an = Int(a.id), bn = Int(b.id)
        switch (an, bn) {
        case (nil, nil): return a.id.localizedCaseInsensitiveCompare(b.id) == .orderedAscending
        case (nil, .some): return true   // letter before number
        case (.some, nil): return false
        case (.some, .some): return an! < bn!
        }
    }
}

// MARK: - Command palette (loaded once from commands.conf)

// Typed command specs. Plain `name = script` lines become .shell commands;
// INI-style sections configure note/list behavior (see commands.conf).
struct CommandSpec {
    enum Kind { case shell, note, list, output, files }

    let name: String
    let kind: Kind
    let windowName: String   // PopupConfig.name -> window title / identity
    let chromeTitle: String  // drag-header label
    let script: String?    // shell: command to run
    let paths: [String]    // note: files edited in-window (tabs when > 1)
    let sources: [String]  // list: JSON array (or TSV) data files (tabs when > 1)
    let root: String?     // files: starting directory for the file browser
    let favorites: [String]  // files: static favorite dirs (commands.conf, tilde ok)
    let zoxideTop: Int       // files: include the top-N dirs from zoxide as favorites
    let browserBackground: NSColor?  // files: panel background (default silvery blue)
    let backgroundColor: NSColor?  // note/files: window card fill (the notepad)
    let tintAlpha: CGFloat?        // note/files: card opacity override (0-1)
    let primary: String?   // list: field shown as the row title
    let content: String?   // list: field drawn next to the title (truncated)
    let detail: String?    // list: field drawn dim on line 2 (left)
    let trailing: String?  // list: field drawn dim on line 2 (right)
    let body: String?      // list: field drawn wrapped under line 2 (2 lines max)
    let filter: [String]   // list: fields matched by the query (default: all)
    let filters: [String]  // list: dropdown filter dimensions (field keys)
    let width: CGFloat     // list/note: popup width override
    let maxRows: Int       // list: max rows shown
    let contentCap: Int    // list: max chars of `content` before truncation
    let bodyLines: Int     // list: max wrapped lines for `body` (0 = framework default)
    let pageSize: Int      // list: rows per page; 0 = no paging ("load more" row)
    let copyFields: [String]  // list: row fields copied as TSV (empty = no copy UI)
    let copyFormat: String    // list: "tsv" (only format today)
    // window behavior, all commands.conf-driven so new windows need no code:
    let checkbox: Bool?       // list: show the copy checkbox column
                              //       (nil = on when copy-fields is set)
    let resize: Bool          // drag edges/corners to resize
    let drag: Bool            // drag the window by its header
    let sticky: Bool          // stay visible when another app takes focus
    let searchWidth: CGFloat  // list: search bar as a fraction of window width
    let maxStretch: CGFloat   // list: cap on per-row stretch when resized big
    let height: CGFloat       // window height in points
    let maxHeight: CGFloat    // cap on the window height (0 = 60% of screen)
    let font: String?         // font family for this window's text
    let headerColor: NSColor? // drag-header tint (nil = window background)
    let voice: Bool           // note: record + transcribe button in the header
    let terminal: Bool        // note: embedded shell drawer at the bottom
    let terminalHeight: CGFloat
    let terminalDir: String?  // note: starting directory for the embedded shell
    let terminalBackground: NSColor?  // note: shell drawer background (silvery blue)
    let icon: NSImage?        // window header glyph (jira/notes/heart/png)
    let saveDir: String       // prettyprint: where "save file" writes (default /tmp/)

    init(name: String, kind: Kind = .shell, windowName: String? = nil,
         chromeTitle: String? = nil, script: String? = nil, paths: [String] = [],
         sources: [String] = [], root: String? = nil,
         favorites: [String] = [], zoxideTop: Int = 0,
         browserBackground: NSColor? = nil,
         backgroundColor: NSColor? = nil,
         tintAlpha: CGFloat? = nil,
         primary: String? = nil,
         content: String? = nil, detail: String? = nil, trailing: String? = nil,
         body: String? = nil, filter: [String] = [], filters: [String] = [],
         width: CGFloat = 0, maxRows: Int = 0, contentCap: Int = 0,
         bodyLines: Int = 0, pageSize: Int = 0, copyFields: [String] = [],
         copyFormat: String = "tsv", checkbox: Bool? = nil, resize: Bool = false,
         drag: Bool = true, sticky: Bool = true, searchWidth: CGFloat = 0,
         maxStretch: CGFloat = 0, height: CGFloat = 0, font: String? = nil,
         headerColor: NSColor? = nil, voice: Bool = false,
         terminal: Bool = false, terminalHeight: CGFloat = 240,
         terminalDir: String? = nil,
         terminalBackground: NSColor? = nil,
         maxHeight: CGFloat = 0,
         icon: NSImage? = nil,
         saveDir: String = "/tmp/") {
        self.name = name
        self.kind = kind
        self.windowName = windowName ?? name
        self.chromeTitle = chromeTitle ?? (windowName ?? name)
        self.script = script
        self.paths = paths
        self.sources = sources
        self.root = root
        self.favorites = favorites
        self.zoxideTop = zoxideTop
        self.browserBackground = browserBackground
        self.backgroundColor = backgroundColor
        self.tintAlpha = tintAlpha
        self.primary = primary
        self.content = content
        self.detail = detail
        self.trailing = trailing
        self.body = body
        self.filter = filter
        self.filters = filters
        self.width = width
        self.maxRows = maxRows
        self.contentCap = contentCap
        self.bodyLines = bodyLines
        self.pageSize = pageSize
        self.copyFields = copyFields
        self.copyFormat = copyFormat
        self.checkbox = checkbox
        self.resize = resize
        self.drag = drag
        self.sticky = sticky
        self.searchWidth = searchWidth
        self.maxStretch = maxStretch
        self.height = height
        self.maxHeight = maxHeight
        self.font = font
        self.headerColor = headerColor
        self.voice = voice
        self.terminal = terminal
        self.terminalHeight = terminalHeight
        self.terminalDir = terminalDir
        self.terminalBackground = terminalBackground
        self.icon = icon
        self.saveDir = saveDir
    }
}

func loadCommands() -> [CommandSpec] {
    // app-level settings first — [app] may sit anywhere in the file
    applyAppConfigFromDisk()
    let path = settings.commandsConfPath
    guard let content = try? String(contentsOfFile: path, encoding: .utf8) else {
        FileHandle.standardError.write(Data("ws: \(commandsConfName) missing — no command palette\n".utf8))
        return []
    }
    var cmds: [CommandSpec] = []
    var section: (name: String, vars: [String: String])?
    func flushSection() {
        guard let s = section else { return }
        switch s.name {
        case "icons":
            // icon overrides for the workspace-switcher rows ([icons] section)
            iconRules = parseIconRules(s.vars)
        case "app":
            // already applied by applyAppConfigFromDisk() — nothing to do
            break
        default:
            // enabled = true is required: no key, no command. Nothing shows
            // unless the section says enabled = true explicitly.
            if s.vars["enabled"] == "true" {
                cmds.append(makeCommand(s.name, s.vars))
            }
        }
        section = nil
    }
    for line in content.split(separator: "\n") {
        let s = line.trimmingCharacters(in: .whitespaces)
        if s.isEmpty || s.hasPrefix("#") { continue }
        if s.hasPrefix("[") && s.hasSuffix("]") {
            flushSection()
            let name = String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
            if !name.isEmpty {
                section = (name, [:])
            }
            continue
        }
        guard let eq = s.firstIndex(of: "=") else { continue }
        let key = s[..<eq].trimmingCharacters(in: .whitespaces)
        let val = s[s.index(after: eq)...].trimmingCharacters(in: .whitespaces)
        if section != nil {
            section?.vars[key] = val
        } else if !key.isEmpty && !val.isEmpty {
            // plain shell command: name = script
            cmds.append(CommandSpec(name: key, kind: .shell, script: val))
        }
    }
    flushSection()
    return cmds
}

private func makeCommand(_ name: String, _ vars: [String: String]) -> CommandSpec {
    let kind: CommandSpec.Kind
    switch vars["type"] ?? "shell" {
    case "note": kind = .note
    case "list": kind = .list
    case "output": kind = .output
    case "files": kind = .files
    default: kind = .shell
    }
    let filter = (vars["filter"] ?? "")
        .split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .filter { !$0.isEmpty }
    let filters = (vars["filters"] ?? "")
        .split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .filter { !$0.isEmpty }
    let width = (vars["width"] ?? "").isEmpty
        ? 0 : CGFloat(Double(vars["width"] ?? "") ?? 0)
    let maxRows = Int(vars["max-rows"] ?? "") ?? 0
    return CommandSpec(
        name: name, kind: kind, windowName: vars["name"], chromeTitle: vars["title"],
        script: vars["script"],
        paths: csv(vars["paths"] ?? vars["path"]),
        sources: csv(vars["sources"] ?? vars["source"]),
        root: vars["root"],
        favorites: csv(vars["favorites"]),
        zoxideTop: Int(vars["zoxide-top"] ?? "") ?? 0,
        browserBackground: hexColor(vars["browser-background"]),
        backgroundColor: hexColor(vars["background-color"]),
        tintAlpha: num(vars["tint-alpha"]) > 0 ? min(num(vars["tint-alpha"]), 1) : nil,
        primary: vars["primary"],
        content: vars["content"], detail: vars["detail"], trailing: vars["trailing"],
        body: vars["body"], filter: filter, filters: filters, width: width,
        maxRows: maxRows,
        contentCap: Int(vars["content-cap"] ?? "") ?? 0,
        bodyLines: Int(vars["body-lines"] ?? "") ?? 0,
        pageSize: Int(vars["page-size"] ?? "") ?? 0,
        copyFields: csv(vars["copy-fields"]),
        copyFormat: vars["copy-format"] ?? "tsv",
        checkbox: tri(vars["checkbox"]),
        resize: tri(vars["resize"]) ?? false,
        drag: tri(vars["drag"]) ?? true,
        sticky: tri(vars["sticky"]) ?? true,
        searchWidth: num(vars["search-width"]),
        maxStretch: num(vars["max-row-stretch"]),
        height: num(vars["height"]),
        font: vars["font"],
        headerColor: hexColor(vars["header-color"]),
        voice: tri(vars["voice"]) ?? false,
        terminal: tri(vars["terminal"]) ?? false,
        terminalHeight: num(vars["terminal-height"]) > 0 ? num(vars["terminal-height"]) : 240,
        terminalDir: vars["terminal-dir"],
        terminalBackground: hexColor(vars["terminal-background"]),
        maxHeight: num(vars["max-height"]),
        icon: vars["icon"].flatMap(resolveIconName),
        saveDir: (vars["save-dir"] ?? "").isEmpty ? "/tmp/" : vars["save-dir"]!)
}

// hex color from commands.conf: "7d8fa6", "0x7d8fa6" or "#7d8fa6" (opaque),
// or 8-digit "aa7d8fa6" / "0xaa7d8fa6" where the leading AA is the ALPHA
// (0x00-0xFF) — the picker's opacity slider is stored that way
private func hexColor(_ s: String?) -> NSColor? {
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
                   blue: Double(v & 0xFF) / 255, alpha: a)
}

// "true/yes/1/on" | "false/no/0/off" | anything else = nil (unset)
private func tri(_ s: String?) -> Bool? {
    switch s?.lowercased() {
    case "true", "yes", "1", "on": return true
    case "false", "no", "0", "off": return false
    default: return nil
    }
}

// Read the [app] section from commands.conf and apply the overrides. Called
// from loadCommands (and from main.swift before any socket ping) so app
// settings are in place before anything else — order-independent of [icons].
func applyAppConfigFromDisk() {
    guard let content = try? String(contentsOfFile: settings.commandsConfPath,
                                    encoding: .utf8) else { return }
    var vars: [String: String] = [:]
    var inApp = false
    for line in content.split(separator: "\n") {
        let s = line.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("[") && s.hasSuffix("]") {
            inApp = s == "[app]"
            continue
        }
        guard inApp, let eq = s.firstIndex(of: "=") else { continue }
        let key = s[..<eq].trimmingCharacters(in: .whitespaces)
        let val = s[s.index(after: eq)...].trimmingCharacters(in: .whitespaces)
        if !key.isEmpty { vars[key] = val }
    }
    parseAppConfig(vars)
}

// [app] section -> AppSettings overrides. Every key is optional; absent keys
// keep the built-in defaults (see AppSettings). Lists are comma-separated,
// paths may use "~".
private func parseAppConfig(_ vars: [String: String]) {
    let str = { (k: String) -> String? in vars[k]?.trimmingCharacters(in: .whitespaces) }
    let list = { (k: String) -> [String] in
        csv(vars[k]).map { $0.hasPrefix("~") ? ($0 as NSString).expandingTildeInPath : $0 }
    }
    if let v = str("shell"), !v.isEmpty { settings.shell = v }
    if let v = str("terminal-font"), !v.isEmpty { settings.terminalFont = v }
    let sa = (vars["shell-args"] ?? "")
        .split(whereSeparator: { $0 == " " || $0 == "\t" })
        .map(String.init)
    if !sa.isEmpty { settings.shellArgs = sa }
    let cli = list("aerospace-cli")
    if !cli.isEmpty { settings.aerospaceCLI = cli }
    let colors = list("color-sources")
    if !colors.isEmpty { settings.colorSources = colors }
    let dirs = list("app-dirs")
    if !dirs.isEmpty { settings.appDirs = dirs }
    if let v = str("jira-icon"), !v.isEmpty { settings.jiraIconName = v }
    if let v = str("notes-icon"), !v.isEmpty { settings.notesIconName = v }
    if let v = str("notes-socket"), !v.isEmpty { settings.notesSocketName = v }
    if let v = str("focus-file"), !v.isEmpty { settings.focusFileName = v }
    if let v = str("focus-bridge"), !v.isEmpty { settings.focusBridgeName = v }
    if let v = str("switcher-name"), !v.isEmpty { settings.switcherWindowName = v }
    if let v = str("detail-name"), !v.isEmpty { settings.detailWindowName = v }
    if let v = str("aerospace-socket"), !v.isEmpty { settings.aerospaceSocketPath = v }
    if let v = str("crash-log"), !v.isEmpty {
        settings.crashLogPath = v.hasPrefix("~") ? (v as NSString).expandingTildeInPath : v
    }
    if let v = str("debug-flag"), !v.isEmpty {
        settings.aeroDebugFlag = v.hasPrefix("~") ? (v as NSString).expandingTildeInPath : v
    }
    if let v = str("aero-log"), !v.isEmpty {
        settings.aeroLog = v.hasPrefix("~") ? (v as NSString).expandingTildeInPath : v
    }
    if let v = str("voice-locale"), !v.isEmpty { settings.voiceLocale = v }
}

// [icons] section -> IconRule list. Line format per app:
//   app-name = title-match:icon, other-title:icon, *:default-icon
// icon names: jira, notes, or a png filename (binDir) / absolute path.
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
    case "heart": return heartIcon
    case "mic", "voice": return micIcon
    default:
        let p = name.hasPrefix("/") ? name : binDir + "/" + name
        return fileIconTile(p, size: appIconSize)
    }
}

private func num(_ s: String?) -> CGFloat {
    CGFloat(Double(s ?? "") ?? 0)
}

// comma-separated config value -> trimmed non-empty list
private func csv(_ s: String?) -> [String] {
    (s ?? "").split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .filter { !$0.isEmpty }
}

// "Last File Write: YYYY-MM-DD HH:MM:SS" for a file, shown under the header
// title as the drag-header's dim metadata line
private let lastWriteFormatter: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "yyyy-MM-dd HH:mm:ss"
    return f
}()

private func lastWriteLabel(_ path: String) -> String {
    // "Last File Write: …" was removed from the header per request — the
    // footer slot stays for transient messages (e.g. voice errors), so this
    // label is intentionally empty.
    ""
}

private func mtime(of path: String) -> Date? {
    guard let attrs = try? FileManager.default.attributesOfItem(atPath: path) else { return nil }
    return attrs[.modificationDate] as? Date
}

// non-editable files opened as notes (PDFs, images, RTF) are shown as a
// read-only preview instead of markdown — and never saved back to
private func noteIsPreview(_ path: String) -> Bool {
    let ext = (path as NSString).pathExtension.lowercased()
    if ext == "pdf" || ext == "rtf" { return true }
    return ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff"].contains(ext)
}

// A plain text-field sheet. NSAlert's accessory NSTextField REFUSES to take
// first responder, so Cmd+V / Cmd+A / Ctrl+V never reach it — this window is
// fully ours, so the field is focused and every editing shortcut just works.
final class TextFieldSheet: NSObject {
    static var live: [TextFieldSheet] = []   // buttons hold targets weakly — retain
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

// Present a single-line text prompt sheet. onResult receives the entered text
// (nil when cancelled).
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
    // async so it's safe when called right as a previous sheet dismisses
    // (e.g. the "Add a note" chooser -> "New Note" second prompt)
    DispatchQueue.main.async {
        panel.beginSheet(sheet)
        sheet.makeFirstResponder(field)
    }
}

// Expand note/list path entries: each may be a single file OR a directory.
// Directories expand to their matching files (sorted, non-hidden), so a
// `paths`/`sources` value can point at a folder and new files appear
// automatically without editing commands.conf. `extensions` limits directory
// listings to those file extensions (lowercased); nil = all files.
// Dismissed notes: a persistent map of absolute note paths the user closed
// with the tab ✕. They stay OUT of the note list even when a `paths = ~/notes`
// directory entry would re-expand them on every launch — without this, a
// closed note keeps getting resynced back as a tab. Explicitly re-opening a
// note (via + / Finder "Open in Notes") removes it from the map again.
enum DismissedNotes {
    private static let store = NSHomeDirectory() + "/.cache/workspace-switcher/dismissed-notes.json"
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
            out.append(p)   // caller creates it if missing
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
    // dedupe, order-preserving: `paths = ~/notes, ~/notes/norika.md` expands the
    // dir to every note INCLUDING norika.md, so the explicit entry would show
    // the same note as two tabs
    var seen = Set<String>()
    return out.filter { seen.insert($0).inserted }
}

// Runs commands through a persistent bash (spawned once at startup) so
// executing a command never pays shell startup cost.
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

// MARK: - Icons

// Config-driven app-icon overrides, parsed from the [icons] section of
// commands.conf. Each rule: app name -> title-substring matches + optional
// "*" default. iconForApp consults these before falling back to the real
// macOS app icon — the workspace-switcher app hosts several windows (notes,
// jira) in ONE process, so its rows need title-based glyphs.
struct IconRule {
    let app: String
    let defaultIcon: NSImage?
    let titleMatches: [(match: String, icon: NSImage)]
}
var iconRules: [IconRule] = []

let appIconSize: CGFloat = 22

// Row rendering constants — these are the workspace switcher's own look; the
// framework knows nothing about them (rows are drawn via popup.onDrawRow).
let rowPillW: CGFloat = 240
let rowPillH: CGFloat = 24
let rowPillBorder: CGFloat = 2
let rowTextX: CGFloat = 18
let rowIconSize: CGFloat = 22
let rowIconX: CGFloat = 44
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

// Our own windows (notes / jira) live in the icon-less workspace-switcher
// app. One factory builds every glyph: app-picker rows get a tinted rounded
// tile, menu-bar status items get a template silhouette. Each app has its own
// symbol AND accent tint, so the two are never confusable in the picker.
let jiraAccent = NSColor(red: 0.36, green: 0.62, blue: 0.95, alpha: 1)   // ticket blue
let notesAccent = NSColor(red: 0.55, green: 0.80, blue: 0.52, alpha: 1)  // notepad green

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


// flat colorful notepad: warm paper, teal binding with rings, slate lines and
// an amber fold — reads in color next to the blue Jira mark in the picker,
// the window header and the menu bar (lockFocus coords are y-up)
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

// icon assets: filenames live in the [app] section (jira-icon/notes-icon),
// resolved against the binary dir; fall back to drawn glyphs if missing
let notesAppIcon = fileIconTile(settings.notesIconPath, size: appIconSize)
    ?? notepadIcon(size: appIconSize)
let jiraAppIcon = fileIconTile(settings.jiraIconPath, size: appIconSize)
    ?? glyphIcon("ticket", fallback: "J", tint: jiraAccent)
// health checks window glyph: a red heart — plain symbol, no tile (the tile
// read as a square box around the icon in the 16px header)
let heartIcon = glyphIcon("heart.fill", fallback: "♥",
                          tint: NSColor.systemRed.withAlphaComponent(0.9),
                          tile: false)
// voice notes glyph: a red recording mic — reads as "capture voice" at a
// glance next to the notepad and ticket marks
let micIcon = glyphIcon("mic.fill", fallback: "🎙",
                        tint: NSColor.systemRed.withAlphaComponent(0.9),
                        tile: false)
// menu-bar glyphs: the Jira mark and the notepad keep their color so they
// pop against the bar; SF Symbol fallbacks stay template-monochrome
let notesMenuGlyph = fileIconTile(settings.notesIconPath, size: 18) ?? notepadIcon(size: 18)
let jiraMenuGlyph = fileIconTile(settings.jiraIconPath, size: 18)
    ?? glyphIcon("ticket", fallback: "J", tint: jiraAccent)
// menu glyphs for the remaining windows (18pt so they fit menu rows)
let micMenuGlyph = glyphIcon("mic.fill", fallback: "🎙",
                             tint: NSColor.systemRed.withAlphaComponent(0.9),
                             size: 18, tile: false)
let heartMenuGlyph = glyphIcon("heart.fill", fallback: "♥",
                               tint: NSColor.systemRed.withAlphaComponent(0.9),
                               size: 18, tile: false)
// single utility glyph for the consolidated menu-bar item
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

// rounded tile around a bitmap asset (the PNG's own alpha does the shaping)
func fileIconTile(_ path: String, size: CGFloat) -> NSImage? {
    guard let src = NSImage(contentsOfFile: path) else { return nil }
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()
    src.draw(in: NSRect(x: 0, y: 0, width: size, height: size),
             from: .zero, operation: .sourceOver, fraction: 1)
    img.unlockFocus()
    return img
}


// JIRA_SITE for the per-row "open in browser" action (config is chmod 600)
let jiraSite: String = {
    let conf = NSString(string: "~/.config/jira/config").expandingTildeInPath
    guard let s = try? String(contentsOfFile: conf, encoding: .utf8) else { return "" }
    for line in s.split(separator: "\n") where line.hasPrefix("JIRA_SITE=") {
        return String(line.dropFirst("JIRA_SITE=".count))
            .trimmingCharacters(in: CharacterSet(charactersIn: "'\""))
    }
    return ""
}()

func iconForApp(_ app: AppInfo) -> NSImage {
    // config-driven overrides (commands.conf [icons]): an app's windows get a
    // custom glyph when their title matches a configured substring
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

// MARK: - Rows (framework PopupRow adapters)

struct WorkspaceRow: PopupRow {
    let title: String
    let icons: [NSImage]
    let trailing: String?

    init(ws: WorkspaceInfo, iconCache: inout [String: NSImage]) {
        title = ws.id
        var imgs: [NSImage] = []
        for app in ws.apps.prefix(rowMaxIcons) {
            // include the window title in the key: our own app hosts several
            // windows (jira/notes) that each need their OWN glyph — keying
            // by app name alone cached one icon for all of them
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
    init(_ c: CommandSpec) { title = "> \(c.name)"; command = c }
}

// Generic row for list commands (jira etc.): primary field as title with the
// content field on the same line (truncated), detail (dim, left) and trailing
// (dim, right) on a second line. `searchText` is the concatenation of the
// command's `filter` fields, used by onFilter.
struct FieldRow: PopupRow {
    let title: String
    let content: String?
    let trailing: String?
    let detail: String?
    let body: String?
    let searchText: String
    let fields: [String: String]   // raw field values (dropdown filter dims)

    var loadMore: Bool { fields["__loadmore"] != nil }
}

// MARK: - Voice notes (record -> Apple speech recognition)

// Pauseable microphone recorder with LIVE transcription: AVAudioEngine feeds
// SFSpeechAudioBufferRecognitionRequest, partial results stream through
// onPartial (the host shows a live draft in the note) and each finalized
// batch lands via onBatch — committed on every pause AND on a ~20s
// continuous-speech threshold, so text appears as you speak. Requires the
// mic + speech usage strings in the Info.plist (embedded via -sectcreate).
final class VoiceRecorder {
    enum State: Int { case idle = 0, recording = 1, paused = 2, transcribing = 3 }
    private(set) var state: State = .idle
    private(set) var elapsed: TimeInterval = 0
    var onStateChange: ((State) -> Void)?
    var onPartial: ((String) -> Void)?     // live draft (updates as you speak)
    var onBatch: ((String) -> Void)?       // finalized batch (commit to note)
    var onError: ((String) -> Void)?
    // live normalized mic level 0-1, ~10x/sec while a session is active
    var onLevel: ((Float) -> Void)?

    private let engine = AVAudioEngine()
    private var recognizer: SFSpeechRecognizer?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    // a batch closed with endAudio() moves here and STAYS alive until its
    // task callback delivers — deallocating a request while its task is
    // pending CANCELS the recognition ("Recognition request was canceled")
    // and the batch text is lost
    private var finalizing: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var ticker: Timer?
    private var batchTimer: Timer?
    private var teardownTimer: DispatchWorkItem?
    private var lastBatchAt: TimeInterval = 0
    private var lastLevelSample: TimeInterval = 0
    private let batchInterval: TimeInterval = 300   // safety net only, never fires
    // during normal dictation. Natural-pause commits come from the on-device
    // recognizer's own silence detection (~2s), exactly like Apple's Dictate.
    // A mid-speech endAudio() made the recognizer finalize with TRUNCATED text
    // (e.g. a lone "H" instead of "Hello") and the following audio was lost —
    // that was the disappearing-text bug.
    // max delay between stop() and the mic being released, even if the
    // recognizer never delivers the final batch (see stop())
    private let stopTeardownTimeout: TimeInterval = 3

    private func ensureAuthorized() -> Bool {
        let mic = AVCaptureDevice.authorizationStatus(for: .audio)
        let speech = SFSpeechRecognizer.authorizationStatus()
        if mic == .authorized && speech == .authorized { return true }
        // Bundled app: let macOS show its own prompt. Safe now that we ship a
        // real .app with the usage strings (the SIGABRT was a BARE-binary TCC
        // bug); the grant then sticks to the bundle id across rebuilds and
        // launch contexts — this is what makes a cold start from Hyper+S work.
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
        // NEVER call requestAccess/requestAuthorization from the app: for a
        // bare (non-bundle) binary TCC can abort the whole process with a
        // "privacy violation" SIGABRT instead of prompting (seen in
        // ~/.cache/ws-crash.log). The deterministic path is granting the
        // binary manually in System Settings; pressing record re-checks.
        let denied = mic == .denied || speech == .denied
            || mic == .restricted || speech == .restricted
        onError?(denied
            ? "microphone/speech access is blocked for this binary — run aerospace/jira/voice-permissions.sh (or add it in System Settings > Privacy & Security > Microphone AND Speech Recognition), then press record again"
            : "microphone/speech permission not granted yet — run aerospace/jira/voice-permissions.sh (or add this binary in System Settings > Privacy & Security > Microphone AND Speech Recognition), then press record again")
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

    // begin a new recognition batch (recording start / resume / threshold)
    private func startBatch() {
        guard let recognizer else { return }
        let r = SFSpeechAudioBufferRecognitionRequest()
        r.shouldReportPartialResults = true
        request = r
        task = recognizer.recognitionTask(with: r) { [weak self] result, error in
            DispatchQueue.main.async {
                guard let self else { return }
                // release THIS request only now: dropping it (or the engine)
                // while the task is pending CANCELS the recognition and the
                // batch text is lost. A closed batch parked in `finalizing`
                // must also survive until its own callback arrives.
                if let result {
                    let text = result.bestTranscription.formattedString
                    if result.isFinal {
                        let isOurs = self.finalizing === r || self.request === r
                        if self.finalizing === r { self.finalizing = nil }
                        let wasCurrent = self.request === r
                        if wasCurrent { self.request = nil; self.task = nil }
                        if isOurs { self.onBatch?(text) }
                        if wasCurrent && self.state == .recording {
                            // The recognizer finalizes a batch ON ITS OWN after
                            // a short silence (on-device dictation, ~10s of
                            // speech then a pause). Without an immediate
                            // restart the request stays nil and EVERYTHING the
                            // user says until the 20s timer fires is dropped by
                            // the tap — the live text "randomly disappears" and
                            // the dictation is never committed. Restart now.
                            self.startBatch()
                            self.lastBatchAt = self.elapsed
                        }
                    } else {
                        // only stream partials from the CURRENT batch: a stale
                        // batch's late partial would overwrite the live draft
                        if self.request === r {
                            self.onPartial?(text)
                        }
                    }
                } else if let error {
                    let wasCurrent = self.request === r
                    if self.finalizing === r { self.finalizing = nil }
                    if wasCurrent { self.request = nil; self.task = nil }
                    if self.state == .recording {
                        // a mid-session failure (e.g. a silent 20s chunk
                        // reporting "no speech detected") must NOT kill the
                        // session or pollute the note: dictation continues
                        if wasCurrent {
                            self.startBatch()
                            self.lastBatchAt = self.elapsed
                        }
                    } else if self.state == .paused {
                        // a pause-finalized chunk erroring must not kill the
                        // paused session either
                    } else {
                        // stop path: a silent recording reports "no speech
                        // detected"; treat a silent/canceled final as an empty
                        // batch (graceful reset) instead of a fatal error
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

    // end the current batch: the recognizer finalizes it and onBatch fires.
    // The request moves to `finalizing` and STAYS alive until its callback
    // delivers — nil'ing it here deallocates it and cancels the task.
    private func finalizeBatch() {
        if let r = request {
            r.endAudio()
            finalizing = r
            request = nil
        }
    }

    private func appendBuffer(_ buffer: AVAudioPCMBuffer) {
        request?.append(buffer)
        // live meter from the tap (RMS -> 0-1), throttled to ~10Hz
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
        // the engine stays hot; the batch finalizes and commits, a new batch
        // starts on resume
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
        // KEEP the engine + request alive: the pending task needs the live
        // audio session to deliver its final result. Stopping the engine /
        // dropping the request here cancels the task ("Recognition request
        // was canceled" / "No speech detected") and the whole batch is lost.
        state = .transcribing
        onStateChange?(state)
        // The host commits the pending final batch via onBatch -> resetSession
        // (which stops the engine + releases the mic). If the recognizer never
        // delivers (hang), this timeout guarantees the teardown anyway. The
        // work item retains self so a late final batch can still commit first.
        teardownTimer?.cancel()
        let item = DispatchWorkItem { [self] in
            self.resetSession()
        }
        teardownTimer = item
        DispatchQueue.main.asyncAfter(deadline: .now() + stopTeardownTimeout,
                                      execute: item)
    }

    // called by the host after the LAST batch commits: stop the engine and
    // return to idle (the note already holds the full transcription)
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

    // Safety net for NON-STOP continuous speech (minutes without a pause):
    // the on-device recognizer's own silence detection commits at natural
    // pauses; this only fires if the user never pauses at all. The interval
    // is so large (5 min) it never truncates a normal dictation.
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

// MARK: - App controller (behavior hooks only; window logic lives in PopupWindow)

final class SwitcherController: NSObject {
    let popup: PopupWindow
    let commandRunner: CommandRunner?
    var workspaces: [WorkspaceInfo] = []
    var commands: [CommandSpec] = []
    var commandMode = false
    var workspaceSelection = 0
    var commandSelection = 0
    var savedWID: String?
    var savedPID: pid_t?
    private var iconCache: [String: NSImage] = [:]
    // All open sub-windows (note editor / jira list). Several can coexist
    // (notes + jira at the same time); each hides on Esc and removes itself.
    var subWindows: [PopupWindow] = []
    // When the user clicks another app, aerospace's on-focus-changed can fire
    // with a lag and write a STALE bridge entry naming one of our windows;
    // the poller would then yank focus back off the app the user just clicked.
    // Suppress self-activation right after a click outside our windows.
    private var lastOtherAppClick: Date?
    private var globalClickMonitor: Any?
    // debounced auto-format for the /prettyprint window (cancelled/re-armed on
    // every keystroke so paste + brief pause renders once)
    private var prettyFormatWorkItem: DispatchWorkItem?
    // interactive color picker state: the shared NSColorPanel previews live while
    // dragging but only PERSISTS on "Apply". Cancelling (panel "x", Esc, or
    // the Cancel button) reverts the window to the color it had on open.
    private weak var pickerWindow: PopupWindow?
    private var pickerRole: PopupWindow.ThemeRole?
    private var pickerSection = ""
    private var pickerHex = ""
    private var pickerOriginal: NSColor?   // color when the picker opened
    private var pickerCommitted = false    // "Apply" clicked before closing
    private var pickerSawVisible = false   // the panel appeared at least once
    private var pickerWatchdog: Timer?
    private var pickerPanelObserver: Any?

    override init() {
        var config = PopupConfig(name: settings.switcherWindowName)
        config.colors = PopupColors(background: BAR, border: BORDER,
                                    text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        config.enableResize = true
        // shrink/grow the window to fit the current row count while typing
        // (e.g. "/" with 3 commands gets a compact window, not a tall one)
        config.dynamicHeight = true
        popup = PopupWindow(config: config)
        commandRunner = CommandRunner()
        super.init()
        commands = loadCommands()

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
            // the toggle path goes through popup.show(), not controller
            // show() — refresh the keypress-time focus target HERE
            (self.savedWID, self.savedPID) = readFocusFile()
            // refresh workspace/window state WITHOUT blocking the main thread
            // (aerospace IPC can stall and beachball the toggle otherwise)
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let fresh = gatherWorkspaces()
                DispatchQueue.main.async { [weak self] in
                    guard let self, !fresh.isEmpty else { return }
                    self.workspaces = fresh
                    if self.popup.isShown, !self.commandMode, self.popup.currentQuery.isEmpty {
                        self.popup.setRows(self.filter(""))
                    }
                }
            }
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
        startFocusPoller()
    }

    // macOS refuses EXTERNAL activation of an accessory app (aerospace's
    // focus raises our window but the app never becomes active, so keyboard
    // focus stays in the previous app and alt-j/k looks "stuck").
    // aerospace/focus-bridge.sh (an on-focus-changed hook) writes the newly
    // focused window id to a file; we watch that file's mtime and activate
    // ourselves from the inside — the one activation path that always works.
    // Event-driven: a stat() per tick, no aerospace IPC, no sketchybar load.
    private var bridgeMtime: (Int, Int)?
    private func startFocusPoller() {
        let path = popupTmpDir() + settings.focusBridgeName
        // Record clicks in OTHER apps so the poller never steals focus back
        // right after the user clicked away (see lastOtherAppClick above).
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
        let t = Timer(timeInterval: focusPollInterval, repeats: true) { [weak self] _ in
            guard let self, !NSApp.isActive,
                  self.subWindows.contains(where: { $0.isShown }) else { return }
            if let last = self.lastOtherAppClick,
               Date().timeIntervalSince(last) < 1.0 { return }
            var st = stat()
            guard stat(path, &st) == 0 else { return }
            let mt = (Int(st.st_mtimespec.tv_sec), Int(st.st_mtimespec.tv_nsec))
            if let prev = self.bridgeMtime, prev.0 == mt.0, prev.1 == mt.1 { return }
            self.bridgeMtime = mt
            guard let raw = try? String(contentsOfFile: path, encoding: .utf8),
                  let id = Int(raw.trimmingCharacters(in: .whitespacesAndNewlines)),
                  let w = self.subWindows.first(where: {
                      $0.isShown && $0.nativeWindow.windowNumber == id
                  }) else { return }
            NSApp.activate(ignoringOtherApps: true)
            w.nativeWindow.makeKeyAndOrderFront(nil)
            self.log("aerospace focused our window \(id) — self-activated")
        }
        RunLoop.main.add(t, forMode: .common)
    }

    // Dedicated notes-only entry point: opens the note window directly with no
    // switcher popup. Guarded: only ONE note window ever exists — re-invoking
    // just focuses it (works from any space).
    func showNotes() {
        if popup.isShown {
            popup.hide(restore: false)
        }
        (savedWID, savedPID) = readFocusFile()
        guard let cmd = commands.first(where: { $0.kind == .note }) else {
            log("notes: no note command configured in \(commandsConfName)")
            return
        }
        // match by NAME: notes / voice / output are all edit-mode windows and
        // a generic editMode match would focus the WRONG window when several
        // are open (e.g. notes hijacking a voice session)
        if let w = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            focusSubWindow(w)
            return
        }
        openNoteWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
    }

    // Finder "Open in Notes" service: focus (or open) the notes window and
    // make the given file the active tab.
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
        if let w = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            focusSubWindow(w)
        } else {
            openNoteWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
        }
        if let w = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            w.onOpenExternalPath?(p)
        }
    }

    // Open a command's window directly (no switcher popup) by its
    // commands.conf section name — e.g. hyper+J -> notes, hyper+N -> jira.
    // Re-invoking focuses the existing window of that type.
    func showCommand(_ name: String) {
        // "voice" is no longer its own window: it aliases the merged
        // notes+voice window (commands.conf [notes] with `voice = true`)
        let name = name == "voice" ? "notes" : name
        if popup.isShown {
            popup.hide(restore: false)
        }
        (savedWID, savedPID) = readFocusFile()
        guard let cmd = commands.first(where: { $0.name == name }) else {
            log("launch: no command named '\(name)' in \(commandsConfName)")
            return
        }
        switch cmd.kind {
        case .note:
            // match by NAME, not editMode: notes / voice / output windows are
            // all edit-mode — a generic editMode match would focus notes when
            // the user asked for voice (and vice versa)
            if let existing = subWindows.first(where: { $0.config.name == cmd.windowName }) {
                focusSubWindow(existing)
                return
            }
            openNoteWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
        case .list:
            focusExistingOrOpen(editMode: false) {
                openListWindow(cmd, restoreWID: savedWID, restorePID: savedPID)
            }
        case .files:
            // single instance, matched by NAME (jira is also editMode=false,
            // so a generic editMode guard could focus the wrong window)
            if let existing = subWindows.first(where: { $0.config.name == cmd.windowName }) {
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

    // Single-instance guard: at most ONE note window and ONE list (jira)
    // window can exist. Opening the same type again focuses the existing one
    // instead of creating a duplicate — windows have .canJoinAllSpaces so they
    // appear on every workspace; ordering them forward brings them to the front.
    private func existingWindow(editMode: Bool) -> PopupWindow? {
        subWindows.first { $0.config.editMode == editMode }
    }

    private func focusExistingOrOpen(editMode: Bool, open: () -> Void) {
        popup.hide(restore: false)
        if let existing = existingWindow(editMode: editMode) {
            focusSubWindow(existing)
            return
        }
        open()
    }

    // Bring a sub-window (note/jira) to the front and make it key. Bringing the
    // window onto the CURRENT workspace is handled by the invoking script
    // (workspace_switcher.sh notes|jira) via aerospace BEFORE pinging us — so
    // the daemon never blocks its main thread on aerospace IPC while focusing.
    private func focusSubWindow(_ w: PopupWindow) {
        if !w.isShown {
            // persistent window hidden by Esc: re-show the SAME instance —
            // its editor text and embedded terminal session are still alive
            w.showPersistent()
        } else {
            let win = w.nativeWindow
            win.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        log("focused existing '\(w.config.name)' window")
    }

    // Unix socket for the isolated command launcher: a message naming a
    // commands.conf section ("notes", "jira", …) opens that window in the
    // running daemon (no second process needed).
    private func startCommandServer() {
        let socketPath = popupTmpDir() + settings.notesSocketName
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            unlink(socketPath)
            let fd = socket(AF_UNIX, SOCK_STREAM, 0)
            guard fd >= 0 else { return }
            var addr = makeUnixSockAddr(socketPath)
            let bound = withUnsafePointer(to: &addr) { ptr -> Bool in
                ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
                }
            }
            guard bound else { close(fd); return }
            listen(fd, 4)
            while true {
                let cfd = Darwin.accept(fd, nil, nil)
                guard cfd >= 0 else { continue }
                // a client that connects and never writes must not wedge the
                // accept loop (a wedged loop saturates the backlog and then
                // blocks every future ping in connect())
                var tv = timeval(tv_sec: Int(serverRecvTimeout), tv_usec: 0)
                setsockopt(cfd, SOL_SOCKET, SO_RCVTIMEO, &tv,
                           socklen_t(MemoryLayout<timeval>.size))
                var buf = [UInt8](repeating: 0, count: 128)
                let n = read(cfd, &buf, buf.count)
                close(cfd)
                if n > 0 {
                    let msg = String(bytes: buf[..<n], encoding: .utf8) ?? ""
                    let name = msg.trimmingCharacters(in: .whitespacesAndNewlines)
                    DispatchQueue.main.async { [weak self] in
                        if name == "notes" {
                            self?.showNotes()
                        } else {
                            self?.showCommand(name)
                        }
                    }
                }
            }
        }
    }

    func show() {
        commandMode = false
        workspaceSelection = 0
        commandSelection = 0
        popup.show()   // onShow refreshes the focus target + dismisses sub-windows
    }

    // MARK: Hooks

    // Row rendering — the workspace switcher's own look (pill + title + app
    // icons + "+N"). The framework only hands us the row rect; rows stretch
    // vertically when the window is resized, so center on rect.midY.
    private func drawRow(_ rect: NSRect, _ row: PopupRow, _ selected: Bool) {
        // scale the row's look with the window (Ctrl/Cmd+± drives config.zoom)
        let z = popup.config.zoom
        let cy = rect.midY
        if selected {
            let pill = NSRect(x: (rect.width - rowPillW * z) / 2, y: cy - rowPillH * z / 2,
                              width: rowPillW * z, height: rowPillH * z)
            let p = NSBezierPath(roundedRect: pill, xRadius: popup.config.buttonRadius * z,
                                 yRadius: popup.config.buttonRadius * z)
            popup.config.colors.accent.withAlphaComponent(0.5).setFill()
            p.fill()
            BORDER.setStroke()
            p.lineWidth = rowPillBorder * z
            p.stroke()
        }
        let titleAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 11 * z), .foregroundColor: TEXT,
        ]
        let title = row.title as NSString
        let ts = title.size(withAttributes: titleAttrs)
        title.draw(at: NSPoint(x: rowTextX * z, y: cy - ts.height / 2),
                   withAttributes: titleAttrs)
        var ix: CGFloat = rowIconX * z
        for img in row.icons {
            popupDrawImage(img, in: NSRect(x: ix, y: cy - rowIconSize * z / 2,
                                           width: rowIconSize * z, height: rowIconSize * z))
            ix += rowIconStride * z
        }
        if let trailing = row.trailing {
            let dimAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 11 * z), .foregroundColor: DIM,
            ]
            let s = trailing as NSString
            let ss = s.size(withAttributes: dimAttrs)
            s.draw(at: NSPoint(x: ix + 2, y: cy - ss.height / 2),
                   withAttributes: dimAttrs)
        }
    }

    private func filter(_ query: String) -> [PopupRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if q.hasPrefix("/") {
            // command palette mode: search commands by what follows the slash
            if !commandMode {
                workspaceSelection = popup.selection
                commandMode = true
                popup.selection = commandSelection
            }
            let sub = String(q.dropFirst()).trimmingCharacters(in: .whitespaces)
            let cmds = PopupFuzzy.filter(commands, query: sub) { $0.name }
            if popup.selection >= cmds.count {
                popup.selection = max(0, cmds.count - 1)
            }
            return cmds.map { CommandRow($0) }
        }
        // workspace mode (slash removed or never typed)
        if commandMode {
            commandSelection = popup.selection
            commandMode = false
            popup.selection = workspaceSelection
        }
        let vis = q.isEmpty
            ? workspaces
            : workspaces.filter { ws in
                ws.id.lowercased().contains(q)
                    || ws.apps.contains { $0.name.lowercased().contains(q) }
            }
        if popup.selection >= vis.count {
            popup.selection = max(0, vis.count - 1)
        }
        return vis.map { WorkspaceRow(ws: $0, iconCache: &iconCache) }
    }

    private func accept(_ row: PopupRow) {
        if let cr = row as? CommandRow {
            switch cr.command.kind {
            case .shell:
                popup.hide(restore: true)
                let cmd = cr.command
                // /prettyprint is a custom command handled in-process (a tiny
                // paste-and-format window), not a shell script
                if cmd.name == "prettyprint" {
                    openPrettyPrintWindow(cmd)
                    break
                }
                commandRunner?.run(cmd.script ?? "") { out in
                    self.log("cmd '\(cmd.name)' -> \(out)")
                }
            case .note:
                // selecting a command creates a NEW window: dismiss the
                // switcher entirely (no breadcrumb) and open a fresh note
                // editor for the file — unless one already exists
                let wid = savedWID
                let pid = savedPID
                focusExistingOrOpen(editMode: true) {
                    openNoteWindow(cr.command, restoreWID: wid, restorePID: pid)
                }
            case .list:
                // same for list windows (jira etc.) — single instance
                let wid = savedWID
                let pid = savedPID
                focusExistingOrOpen(editMode: false) {
                    openListWindow(cr.command, restoreWID: wid, restorePID: pid)
                }
            case .output:
                popup.hide(restore: true)
                openOutputWindow(cr.command)
            case .files:
                popup.hide(restore: false)
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

    // MARK: Command actions

    private func log(_ s: String) {
        FileHandle.standardError.write(Data("ws: \(s)\n".utf8))
    }

    // one pasteboard write + log line behind every "copy …" affordance
    private func copy(_ text: String, _ what: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        log("copied \(what)")
    }

    // Menu-bar glyphs toggle their window: hide when it is already the key
    // window, otherwise focus-or-open (same path as the hotkeys).
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
        // "voice" aliases the merged notes+voice window (see showCommand)
        let name = name == "voice" ? "notes" : name
        if let w = subWindows.first(where: { $0.config.name == name }),
           w.isShown, w.nativeWindow.isKeyWindow {
            w.hide(restore: true)
            return
        }
        showCommand(name)
    }

    // Esc/close on a sub-window: drop it from the registry and hand focus back
    // to whatever window was focused when it opened.
    private func unregisterSubWindow(_ w: PopupWindow, restore: Bool,
                                     restoreWID: String?, restorePID: pid_t?) {
        subWindows.removeAll { $0 === w }
        // drop the host hooks so the window + its captured objects (e.g. the
        // voice recorder's AVAudioEngine, which holds the mic) dealloc — a
        // leaked engine made the next voice session's record button dead
        w.releaseHooks()
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

    // "more details": a minimal read-only floating window that renders ONE
    // jira blown up — every field, nothing truncated or wrapped to 2 lines.
    // Esc dismisses. Re-invoking refreshes the single existing detail window.
    private func showDetail(_ row: FieldRow, cmd: CommandSpec) {
        let key = row.fields["key"] ?? row.title
        let text = detailText(for: row, cmd: cmd)
        if let existing = subWindows.first(where: { $0.config.name == settings.detailWindowName }) {
            existing.setEditorText(text)
            existing.chromeHeaderTitle = key
            existing.nativeWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }
        var cfg = PopupConfig(name: settings.detailWindowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableDrag = true
        cfg.sticky = true
        cfg.width = defaultDetailSize.width
        cfg.height = defaultDetailSize.height
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        let w = PopupWindow(config: cfg)
        w.editorReadOnly = true
        w.editorText = text
        w.chromeHeaderTitle = key
        w.headerIcon = jiraAppIcon
        // no config button here — the header carries "copy key" + "open in
        // browser" instead (the row-level browser action now lives here)
        w.copyConfigButtonLabel = ""
        w.copyPathButtonLabel = "copy key"
        w.headerButtons = [("open in browser", 10)]
        w.onChromeHeaderClick = { [weak self] in
            self?.copy(key, "jira key: \(key)")
        }
        w.onHeaderButton = { [weak self] id in
            guard id == 10, let self else { return }
            let url = URL(string: jiraSite + "/browse/" + key)
            if let url {
                NSWorkspace.shared.open(url)
                self.log("detail: opened \(key) in browser")
            }
        }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: nil, restorePID: nil)
        }
        subWindows.append(w)
        w.show()
        log("detail window opened for \(key)")
    }

    // headline fields in commands.conf order, then every remaining raw field
    private func detailText(for row: FieldRow, cmd: CommandSpec) -> String {
        let shown = [cmd.primary, cmd.content, cmd.detail, cmd.trailing, cmd.body]
            .compactMap { $0 }
        var out: [String] = []
        for k in shown {
            if let v = row.fields[k], !v.isEmpty {
                out.append("\(k): \(v)")
                out.append("")
            }
        }
        out.append("--- all fields ---")
        for (k, v) in row.fields.sorted(by: { $0.key < $1.key })
        where !k.hasPrefix("__") && !shown.contains(k) && !v.isEmpty {
            out.append("\(k): \(v)")
        }
        return out.joined(separator: "\n")
    }

    // type = output: run a shell command and show its output in a read-only
    // floating window (the /health-checks palette entry). Re-invoking
    // re-runs into the same window; Esc dismisses.
    private func openOutputWindow(_ cmd: CommandSpec) {
        if let existing = subWindows.first(where: { $0.config.name == cmd.windowName }) {
            existing.nativeWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            runOutput(cmd, into: existing)
            return
        }
        var cfg = PopupConfig(name: cmd.windowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.width = cmd.width > 0 ? cmd.width : defaultOutputSize.width
        cfg.height = cmd.height > 0 ? cmd.height : defaultOutputSize.height
        // same header styling as the jira window: slim bluey-silver bar, no
        // title pill, jira glyph at the far left
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = cmd.headerColor ?? headerBlueSilver
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        cfg.fontName = cmd.font
        let w = PopupWindow(config: cfg)
        w.editorReadOnly = true
        w.editorText = "running \(cmd.name)…"
        // empty `title` in commands.conf = no header label (icon still shows)
        w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
        // per-command header glyph (commands.conf `icon`); jira by default
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
        w.show()
        runOutput(cmd, into: w)
    }

    private func runOutput(_ cmd: CommandSpec, into w: PopupWindow) {
        runScript(cmd.script ?? "", label: cmd.name, into: w)
    }

    // /prettyprint: a tiny paste-and-format window. Paste raw JSON/XML into
    // the editor and it re-renders itself formatted (debounced as you paste);
    // the "copy contents" header button copies the contents, "save file" writes
    // it out (to the command's `save-dir`) and copies the absolute path.
    private func openPrettyPrintWindow(_ cmd: CommandSpec) {
        let windowName = "prettyprint"
        if let existing = subWindows.first(where: { $0.config.name == windowName }) {
            focusSubWindow(existing)
            return
        }
        var cfg = PopupConfig(name: windowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableDrag = true
        cfg.sticky = true
        cfg.width = 1000
        cfg.height = 600
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = headerBlueSilver
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        let w = PopupWindow(config: cfg)
        w.editorReadOnly = false
        w.editorText = ""
        w.chromeHeaderTitle = nil
        w.headerIcon = notesAppIcon
        w.itemCount = "paste JSON or XML below — auto-formats"
        // hide the framework's copy config; keep our own two buttons
        w.copyConfigButtonLabel = ""
        w.copyPathButtonLabel = ""
        w.headerButtons = [("save file", 11), ("copy contents", 10)]
        w.onEditorTextChange = { [weak self, weak w] in
            guard let self, let w else { return }
            self.prettyFormatWorkItem?.cancel()
            let work = DispatchWorkItem { [weak self, weak w] in
                guard let self, let w else { return }
                let raw = w.currentEditorText
                // formatting shells out to jq/xmllint — keep it off the main
                // thread so a big document never stalls the UI
                DispatchQueue.global(qos: .userInitiated).async { [weak self, weak w] in
                    let result = self?.prettyFormat(raw)
                    DispatchQueue.main.async { [weak w] in
                        guard let result, let w else { return }
                        if let formatted = result.formatted {
                            // render the formatted text with JSON/XML token
                            // colors (idempotent — re-applied even when the
                            // text is unchanged so colors never go stale)
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
                // copy contents
                let contents = w.currentEditorText
                guard !contents.isEmpty else { return }
                self.copy(contents, "prettyprint contents")
            } else if id == 11 {
                // save to a file + copy the absolute path
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
        log("prettyprint window opened")
    }

    // Timestamp for prettyprint filenames: prettyprint-20260918-173045.json
    private let prettySaveStamp: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "yyyyMMdd-HHmmss"
        return f
    }()

// "save file": write the current editor contents to a timestamped file in the
// command's configured save directory (default /tmp/; commands.conf `save-dir`
// overrides), then copy the ABSOLUTE path to the clipboard (pbcopy equivalent).
// Feedback lands in the status strip — the saved path on success, a red error
// on failure.
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
    // expand any `~`/relative path into an absolute directory
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
    log("prettyprint saved to \(path)")
}

// Sniff + reformat JSON/XML via the real formatter bins (jq / xmllint — the
// same tools the shell `prettyprint` util uses), so the output AND the parse
// errors match exactly. `.formatted` = pretty text (content was JSON/XML);
// `.error` = the formatter's stderr (content LOOKED like JSON/XML but didn't
// parse); nil/nil = plain text (nothing to do).
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

// Run a formatter tool with `input` on stdin; returns (stdout, stderr). Pipes
// are drained on a background queue so large output can't deadlock the read.
// Falls back to PATH lookup (/usr/bin/env) when none of the absolute paths
// exist, so the tool is found however the daemon was launched.
private func runFormatter(tool: String, paths: [String], args: [String],
                          input: String) -> (String, String) {
    var executable = URL(fileURLWithPath: "/usr/bin/env")
    var argv = [tool] + args
    if let p = paths.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) {
        executable = URL(fileURLWithPath: p)
        argv = args
    }
    let p = Process()
    p.executableURL = executable
    p.arguments = argv
    let inp = Pipe()
    let out = Pipe()
    let err = Pipe()
    p.standardInput = inp
    p.standardOutput = out
    p.standardError = err
    do { try p.run() } catch { return ("", "\(tool): \(error.localizedDescription)") }
    var outData = Data()
    var errData = Data()
    let group = DispatchGroup()
    group.enter()
    DispatchQueue.global(qos: .userInitiated).async {
        outData = out.fileHandleForReading.readDataToEndOfFile()
        group.leave()
    }
    group.enter()
    DispatchQueue.global(qos: .userInitiated).async {
        errData = err.fileHandleForReading.readDataToEndOfFile()
        group.leave()
    }
    inp.fileHandleForWriting.write(input.data(using: .utf8) ?? Data())
    try? inp.fileHandleForWriting.close()
    p.waitUntilExit()
    group.wait()
    return (String(data: outData, encoding: .utf8) ?? "",
            String(data: errData, encoding: .utf8) ?? "")
}

private func trimmed(_ s: String) -> String? {
    let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
    return t.isEmpty ? nil : t
}

    // run any shell line into an output window
    private func runScript(_ script: String, label: String, into w: PopupWindow) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self, weak w] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: settings.shell)
            p.arguments = ["-c", (script as NSString).expandingTildeInPath]
            let out = Pipe()
            let err = Pipe()
            p.standardOutput = out
            p.standardError = err
            do {
                try p.run()
            } catch {
                DispatchQueue.main.async { w?.setEditorText("failed to run: \(error)") }
                return
            }
            let text = String(data: out.fileHandleForReading.readDataToEndOfFile(),
                              encoding: .utf8) ?? ""
            let etext = String(data: err.fileHandleForReading.readDataToEndOfFile(),
                               encoding: .utf8) ?? ""
            p.waitUntilExit()
            // render ANSI colors (doctor's PASS/FAIL/WARN) in the editor
            var shown = text
            if !etext.isEmpty {
                shown += "\n-- stderr --\n" + etext
            }
            shown += "\n(exit \(p.terminationStatus))"
            DispatchQueue.main.async { [weak self] in
                guard let w else { return }
                w.setEditorANSI(shown)
                self?.log("output '\(label)': exit \(p.terminationStatus)")
            }
        }
    }

    // note: edit the file(s) in-window (no external editor). Files are created
    // if missing, saved on Cmd+S and whenever the window closes. With multiple
    // `paths`, each note pad is a tab.
    private func openNoteWindow(_ cmd: CommandSpec,
                                restoreWID: String?, restorePID: pid_t?) {
        log("openNoteWindow: '\(cmd.name)' terminal=\(cmd.terminal) paths=\(cmd.paths.count)")
        guard !cmd.paths.isEmpty else {
            log("note '\(cmd.name)': no path configured")
            return
        }
        // expand ~, and expand any directory entry to its matching files (sorted).
    // A `paths`/`sources` value may be a single file OR a directory — pointing
    // at a folder means new files show up automatically without editing
    // commands.conf. Deleted notes are NOT resurrected: a listed file that no
    // longer exists is dropped (and removed from commands.conf) instead of
    // being recreated empty.
    var paths: [String] = []
    for p in expandPaths(cmd.paths, extensions: ["md"]) {
        if !FileManager.default.fileExists(atPath: p) {
            log("note '\(cmd.name)': \(p) deleted — dropping it and removing from config")
            removeNotePathFromConfig(p, section: cmd.name)
        } else if DismissedNotes.contains(p) {
            // closed with the tab ✕ — keep it out even if a directory entry
            // (e.g. paths = ~/notes) would otherwise re-expand it here
            log("note '\(cmd.name)': \(p) dismissed — skipping")
        } else {
            paths.append(p)
        }
    }
    if paths.isEmpty {
        // every listed note is gone — open a fresh default.md scratch note
        let first = (cmd.paths[0] as NSString).expandingTildeInPath
        let fallback = (first as NSString).deletingLastPathComponent + "/default.md"
        try? FileManager.default.createDirectory(
            atPath: (fallback as NSString).deletingLastPathComponent,
            withIntermediateDirectories: true)
        if !FileManager.default.fileExists(atPath: fallback) {
            FileManager.default.createFile(atPath: fallback, contents: nil)
        }
        paths = [fallback]
        addNotePathToConfig(fallback, section: cmd.name)
        log("note '\(cmd.name)': all listed notes deleted — opened fresh \(fallback)")
    }
    var titles = paths.map { URL(fileURLWithPath: $0).lastPathComponent }
    var currentPath = paths[0]
    let content = (try? String(contentsOfFile: currentPath, encoding: .utf8)) ?? ""

        var cfg = PopupConfig(name: cmd.windowName)
        cfg.enableToggle = false
        cfg.editMode = true
        cfg.enableResize = cmd.resize
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.tabs = true
        cfg.tabsAddButton = true
        cfg.width = cmd.width > 0 ? cmd.width : defaultNoteSize.width
        // the file browser is the default pane (open on launch); the terminal
        // starts closed — the initial height folds in whichever drawer opens
        cfg.fileBrowserDefault = true
        cfg.height = (cmd.height > 0 ? cmd.height : defaultNoteSize.height)
            + (cfg.fileBrowserDefault ? cfg.fileBrowserHeight
                                      : (cmd.terminal ? cfg.terminalHeight : 0))
        if cmd.maxHeight > 0 { cfg.maxHeight = cmd.maxHeight }
        cfg.terminal = cmd.terminal
        cfg.terminalHeight = cmd.terminalHeight
        if let td = cmd.terminalDir { cfg.terminalDir = td }
        cfg.fileBrowserBackground = cmd.browserBackground
            ?? THEME_BROWSER ?? cfg.fileBrowserBackground
        cfg.shell = settings.shell
        cfg.shellArgs = settings.shellArgs
        cfg.terminalFont = settings.terminalFont
        cfg.terminalBackground = cmd.terminalBackground
            ?? THEME_TERMINAL ?? cfg.terminalBackground
        // slim header (same height as the jira detail window): no title pill,
        // bluey-silver strip, app glyph far left with the last-write line
        cfg.headerHeight = 30
        cfg.titlePill = false
        // the header buttons fill the whole top strip (right rounded edge back
        // to the app glyph / last-write line) instead of a compact right cluster
        cfg.stretchHeaderButtons = true
        cfg.headerColor = cmd.headerColor ?? headerBlueSilver
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        if let ta = cmd.tintAlpha { cfg.tintAlpha = ta }
        if let bg = cmd.backgroundColor {
            let cc = bg.usingColorSpace(.sRGB) ?? bg
            // the card hue stays opaque; the color's alpha becomes the card
            // opacity (tintAlpha) so the picker's opacity slider survives
            cfg.colors.background = cc.withAlphaComponent(1)
            cfg.tintAlpha = cc.alphaComponent
        }
        cfg.fontName = cmd.font
        cfg.markdownImages = true
        let w = PopupWindow(config: cfg)
        // header buttons: "\u{F120}" (terminal icon) toggles the embedded shell
        // drawer; "\u{F0036}" (Nerd Fonts "fa-blackberry", matches the
        // installed 3.5.1 font) toggles the file browser. Opening files
        // happens via the "+" tab button.
        var hb: [(String, Int)] = []
        if cmd.terminal { hb.append(("\u{F120}", 10)) }
        hb.append(("\u{F0036}", 20))
        // voice windows get a mic toggle (id 40) that shows/hides the record
        // bar — clustered with the terminal + folder toggles. The bar starts
        // OFF (slashed mic), so recording never starts silently at launch.
        if cmd.voice { hb.append(("\u{F131}", 40)) }
        // paint-brush (id 60): opens the interactive color picker that edits
        // the silvery-blue panel background (terminal + file browser) live
        hb.append(("\u{F1FC}", 60))
        w.headerButtons = hb
        w.headerOrder = [1, 2]
        // initial drawer state: terminal starts on, browser starts off — the
        // header buttons mirror that; the record bar starts hidden for voice
        if cmd.terminal { w.setHeaderButtonOn(10, false) }
        w.setHeaderButtonOn(20, false)
        if cmd.voice { w.setHeaderButtonOn(40, false) }
        func noteDir(_ p: String) -> String { (p as NSString).deletingLastPathComponent }
        w.imageBaseDir = noteDir(currentPath)
        if noteIsPreview(currentPath) {
            w.setEditorFilePreview(currentPath)   // PDF / image: read-only preview
        } else {
            w.editorReadOnly = false
            w.setEditorMarkdown(content, baseDir: noteDir(currentPath))
        }
        // pasted/dropped photos land in <note dir>/assets and render inline
        w.imageSaver = { [weak self] img in
            guard let self else { return nil }
            let dir = noteDir(currentPath) + "/assets"
            try? FileManager.default.createDirectory(atPath: dir,
                                                     withIntermediateDirectories: true)
            let name = "img-\(Int(Date().timeIntervalSince1970)).png"
            guard let tiff = img.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff),
                  let data = rep.representation(using: .png, properties: [:]) else { return nil }
            do {
                try data.write(to: URL(fileURLWithPath: dir + "/" + name))
                self.log("note '\(cmd.name)': saved pasted image assets/\(name)")
                return "assets/" + name
            } catch {
                self.log("note '\(cmd.name)': image save failed: \(error)")
                return nil
            }
        }
        // per-command header glyph (commands.conf `icon`); notepad by default
        w.headerIcon = cmd.icon ?? notesAppIcon
        // empty `title` in commands.conf = no header label (icon still shows)
        w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
        w.copyPathButtonLabel = ""          // copy path moved to right-click (tab/editor)
        w.copyConfigButtonLabel = ""        // config is opened via the icon click
        w.tabTitles = titles
        w.tabFooterText = lastWriteLabel(currentPath)
        // generic label in the drag header — the tab strip already shows the
        // individual note names; clicking the header still copies the path
        w.chromeHeaderTitle = cmd.chromeTitle
        // external-write watch state: reload the current note when its file
        // changes on disk, unless the editor holds unsaved local edits
        var lastSynced = content
        var lastMtime = mtime(of: currentPath)
        var watcher: Timer?
        // switching tabs: save the current note, load the new one. A tab whose
        // note was deleted on disk becomes default.md instead (never recreate)
        let loadTab: (Int) -> Void = { [weak self] index in
            guard let self, index < paths.count else { return }
            let outgoing = currentPath
            // save the outgoing note BEFORE the editor is swapped to the new
            // tab — after setEditorMarkdown, currentEditorText would already
            // hold the NEW note's content and overwrite (wipe) the outgoing
            // file. Never resurrect a deleted file: a missing outgoing is
            // skipped (its text was already parked by the watcher/commit).
            if FileManager.default.fileExists(atPath: outgoing), !noteIsPreview(outgoing) {
                self.saveNote(w.currentEditorText, to: outgoing, cmd: cmd)
            }
            var target = paths[index]
            if !FileManager.default.fileExists(atPath: target) {
                let fallback = noteDir(target) + "/default.md"
                try? FileManager.default.createDirectory(atPath: noteDir(fallback),
                                                         withIntermediateDirectories: true)
                if !FileManager.default.fileExists(atPath: fallback) {
                    FileManager.default.createFile(atPath: fallback, contents: nil)
                }
                self.log("note '\(cmd.name)': \(target) deleted — tab now default.md")
                paths[index] = fallback
                titles[index] = URL(fileURLWithPath: fallback).lastPathComponent
                w.tabTitles = titles
                self.removeNotePathFromConfig(target, section: cmd.name)
                self.addNotePathToConfig(fallback, section: cmd.name)
                target = fallback
            }
            currentPath = target
            w.imageBaseDir = noteDir(currentPath)
            if noteIsPreview(currentPath) {
                // PDF / image: read-only preview, never editable text
                w.setEditorFilePreview(currentPath)
                lastSynced = ""
            } else {
                w.editorReadOnly = false
                let loaded = (try? String(contentsOfFile: currentPath, encoding: .utf8)) ?? ""
                w.setEditorMarkdown(loaded, baseDir: noteDir(currentPath))
                lastSynced = loaded
            }
            lastMtime = mtime(of: currentPath)
            w.tabFooterText = lastWriteLabel(currentPath)
            w.copyPathButtonLabel = ""          // path lives on the right-click
            w.onChromeHeaderClick = {
                self.copy(currentPath, "note path: \(currentPath)")
            }
        }
        w.onTabChange = { [weak self] index in
            guard let self else { return }
            loadTab(index)
        }
        // tab "✕": close the note at `index`. It is dropped from the tab list
        // and from commands.conf so it never shows up as a note again — the
        // file itself stays on disk untouched. Closing the last note opens a
        // fresh default.md scratch pad next to it.
        let closeNote: (Int) -> Void = { [weak self, weak w] index in
            guard let self, let w else { return }
            guard paths.indices.contains(index) else { return }
            let closing = paths[index]
            let wasCurrent = index == w.selectedTab
            // only save when closing the ACTIVE tab — otherwise the editor
            // holds a different note's text and must not touch this file
            // (preview files like PDFs are never written back)
            if wasCurrent, FileManager.default.fileExists(atPath: closing),
               !noteIsPreview(closing) {
                self.saveNote(w.currentEditorText, to: closing, cmd: cmd)
            }
            self.log("note '\(cmd.name)': closed \(closing)")
            DismissedNotes.add(closing)
            self.removeNotePathFromConfig(closing, section: cmd.name)
            paths.remove(at: index)
            titles.remove(at: index)
            if paths.isEmpty {
                let fallback = noteDir(closing) + "/default.md"
                try? FileManager.default.createDirectory(
                    atPath: noteDir(fallback), withIntermediateDirectories: true)
                if !FileManager.default.fileExists(atPath: fallback) {
                    FileManager.default.createFile(atPath: fallback, contents: nil)
                }
                paths = [fallback]
                titles = [URL(fileURLWithPath: fallback).lastPathComponent]
                self.addNotePathToConfig(fallback, section: cmd.name)
            }
            w.tabTitles = titles
            if wasCurrent {
                // switch to the tab that slid into this slot (or the last one)
                let next = min(index, paths.count - 1)
                if w.selectedTab == next {
                    loadTab(next)          // same slot value — reload manually
                } else {
                    w.selectedTab = next   // fires onTabChange -> loadTab
                }
            } else if index < w.selectedTab {
                // the closed tab was before the selection — it slid down one
                w.selectedTab -= 1         // fires onTabChange (same note)
            }
        }
        // "✕" on a tab pill closes that note (removes it from the list)
        w.onCloseTab = { [weak self] index in
            guard let self else { return }
            closeNote(index)
        }
        // header button routing (terminal / folder / mic / color picker)
        w.onHeaderButton = { [weak self, weak w] id in
            if id == 10 {
                w?.toggleTerminalDrawer()
                if let w { w.setHeaderButtonOn(10, w.terminalShown) }
            } else if id == 20 {
                w?.toggleFileBrowser()
                if let w { w.setHeaderButtonOn(20, w.fileBrowserShown) }
            } else if id == 40 {
                guard let w else { return }
                let shown = !w.meterEnabled
                w.meterEnabled = shown
                w.setHeaderButtonOn(40, shown)
                // swap the mic glyph: solid mic when the bar is shown,
                // slashed mic when hidden, so the state reads at a glance
                var arr = w.headerButtons
                if let i = arr.firstIndex(where: { $0.1 == 40 }) {
                    arr[i].0 = shown ? "\u{F130}" : "\u{F131}"
                    w.headerButtons = arr
                }
            } else if id == 60 {
                guard let w else { return }
                self?.presentThemeRoleMenu(for: w,
                                           roles: [.terminal, .browser, .notepad, .header],
                                           section: cmd.name)
            }
        }
        // "+" pill: choose to open an EXISTING file as a tab (open panel) or
        // create a NEW note in the default dir (next to the first note). Both
        // are presented as SHEETs on the note window so they always appear in
        // front.
        w.onAddTab = { [weak self] in
            guard let self else { return }
            let panel = w.nativeWindow
            panel.makeKeyAndOrderFront(nil)
            let chooser = NSAlert()
            chooser.messageText = "Add a note"
            chooser.informativeText = "Open an existing file, or create a new note:"
            chooser.addButton(withTitle: "Open Existing…")
            chooser.addButton(withTitle: "New Note")
            chooser.addButton(withTitle: "Cancel")
            chooser.beginSheetModal(for: panel) { [weak self] response in
                guard let self else { return }
                switch response {
                case .alertFirstButtonReturn:
                    // "Open Existing…": no Finder picker — the integrated file
                    // browser below is the picker. Just ask for a path and
                    // trust it; if it isn't a real file the note doesn't open.
                    w.onOpenPathPrompt?()
                case .alertSecondButtonReturn:
                    // "New Note": prompt for a name, create in the default dir
                    presentPathSheet(on: panel,
                                     title: "New note",
                                     message: "Name for the new note:",
                                     okTitle: "Create") { [weak self] value in
                        guard let self, let value else { return }
                        var name = value.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !name.isEmpty else {
                            self.log("note '\(cmd.name)': empty name, not creating")
                            return
                        }
                        if !name.hasSuffix(".md") { name += ".md" }
                        let baseDir = (paths[0] as NSString).deletingLastPathComponent
                        let newPath = baseDir + "/" + name
                        // already open (in memory)? just switch to that tab — never
                        // duplicate a note that already exists
                        if let idx = paths.firstIndex(of: newPath) {
                            w.selectedTab = idx
                            return
                        }
                        if !FileManager.default.fileExists(atPath: newPath) {
                            FileManager.default.createFile(atPath: newPath, contents: nil)
                        }
                        paths.append(newPath)
                        titles.append(URL(fileURLWithPath: newPath).lastPathComponent)
                        w.tabTitles = titles
                        w.selectedTab = paths.count - 1   // fires onTabChange -> loads it
                        DismissedNotes.remove(newPath)
                        self.addNotePathToConfig(newPath, section: cmd.name)
                        self.log("note '\(cmd.name)': created \(newPath)")
                    }
                default:
                    break
                }
            }
        }
        // clicking the ACTIVE note tab copies that note's absolute path
        w.onTabClick = { [weak self] index in
            guard let self, index == w.selectedTab, index < paths.count else { return }
            self.copy(paths[index], "note path: \(paths[index])")
        }
        // right-click a note TAB -> copy that note's absolute path
        w.onTabCopyPath = { [weak self] index in
            guard let self, index < paths.count else { return }
            self.copy(paths[index], "note path: \(paths[index])")
        }
        // right-click the editor -> "Copy File Path" copies the open note
        w.onCopyFilePath = { [weak self] in
            guard let self else { return }
            self.copy(currentPath, "note path: \(currentPath)")
        }
        // Finder "Open in Notes" service: open an arbitrary file as a tab and
        // switch to it (the file already exists on disk — never create it)
        w.onOpenExternalPath = { [weak self, weak w] path in
            guard let self, let w else { return }
            let p = (path as NSString).standardizingPath
            if let idx = paths.firstIndex(of: p) {
                w.selectedTab = idx
                return
            }
            guard FileManager.default.fileExists(atPath: p) else {
                self.log("note '\(cmd.name)': cannot open \(p) — missing")
                return
            }
            paths.append(p)
            titles.append(URL(fileURLWithPath: p).lastPathComponent)
            w.tabTitles = titles
            w.selectedTab = paths.count - 1   // fires onTabChange -> loads it
            DismissedNotes.remove(p)          // explicit re-open beats the ✕
            self.addNotePathToConfig(p, section: cmd.name)
            self.log("note '\(cmd.name)': opened \(p)")
        }
        // editor context menu -> "Open file at path…": prompt for an exact
        // path and open it as a tab
        w.onOpenPathPrompt = { [weak self, weak w] in
            guard let self, let w else { return }
            let panel = w.nativeWindow
            presentPathSheet(on: panel,
                             title: "Open file at path",
                             message: "Absolute path (or ~/…) to open as a note:",
                             okTitle: "Open") { [weak self, weak w] value in
                guard let self, let w, let value else { return }
                let raw = value.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !raw.isEmpty else { return }
                let p = ((raw as NSString).expandingTildeInPath as NSString).standardizingPath
                if FileManager.default.fileExists(atPath: p) {
                    w.onOpenExternalPath?(p)
                } else {
                    self.log("note '\(cmd.name)': no such path \(p)")
                }
            }
        }
        // terminal drawer right-click "Open in Notes": the selected text is a
        // path — open it as a note tab (openNoteFile checks it exists)
        w.onTerminalOpenInNotes = { [weak self] path in
            self?.openNoteFile(path)
        }
        // terminal right-click "Open in Default App" / "Reveal in Finder":
        // act on the selected path (existence-checked before acting)
        w.onTerminalOpenDefault = { path in
            guard FileManager.default.fileExists(atPath: path) else { return }
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
        w.onTerminalRevealInFinder = { path in
            guard FileManager.default.fileExists(atPath: path) else { return }
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
        }
        w.onChromeHeaderClick = { [weak self] in
            self?.copy(currentPath, "note path: \(currentPath)")
        }
        // clicking the top-left notes glyph opens commands.conf as a note tab
        // (the dedicated "copy config path" button was dropped for this)
        w.onChromeIconClick = { [weak self, weak w] in
            guard let self, let w else { return }
            // the CANONICAL user-facing config path (~/.config/workspace-switcher/
            // commands.conf, the INSTALL.sh symlink target); fall back to the
            // binary-relative one if the symlink is absent
            let canonical = NSHomeDirectory() + "/.config/workspace-switcher/commands.conf"
            let p = FileManager.default.fileExists(atPath: canonical)
                ? canonical
                : settings.commandsConfPath
            if FileManager.default.fileExists(atPath: p) {
                w.onOpenExternalPath?(p)
            } else {
                self.log("note '\(cmd.name)': config not found at \(p)")
            }
        }
        // voice notes (commands.conf `voice = true`): the window's bottom bar
        // becomes a record control — big record/stop button, pause/resume and
        // live level bars; stopping transcribes with Apple's speech
        // recognizer and appends a dated block to the current note. The
        // header keeps its copy-path / copy-config buttons (notes + voice
        // share this window).
        if cmd.voice {
            self.log("voice '\(cmd.name)': voice controls enabled")
            let voice = VoiceRecorder()
            // record bar starts OFF (meterEnabled stays false) — the user
            // toggles it on via the header mic button when they want it
            // Bulletproof session model: while recording, the editor and the
            // file are ALWAYS rebuilt as
            //     immutable + committedStr + liveDraft
            // from parts that only ever GROW. Nothing slices disk prefixes,
            // nothing prefix-matches strings, and offsets are UTF-16-safe —
            // so committed text cannot vanish no matter how the recognizer
            // batches, pauses, errors or races.
            var immutable = ""        // editor text captured at record start
            var immutableDisk = ""    // file content captured at record start
            var committedStr = ""     // finalized batches this session (grown)
            var draft = ""            // live partial hypothesis (transient)
            var liveWrite: Timer?
            let dbgPath = NSString(string: "~/.cache/ws-voice-debug.log")
                .expandingTildeInPath
            func dbg(_ s: String) {
                let line = "\(Date()) \(s)\n"
                if let h = FileHandle(forWritingAtPath: dbgPath) {
                    h.seekToEndOfFile()
                    h.write(line.data(using: .utf8)!)
                    h.closeFile()
                } else {
                    FileManager.default.createFile(atPath: dbgPath,
                                                   contents: line.data(using: .utf8))
                }
            }
            func sep0() -> String { immutable.isEmpty ? "" : "\n\n" }
            func immLen() -> Int { (immutable as NSString).length }
            func committedLen() -> Int { (committedStr as NSString).length }
            // committed + live draft, with separators
            func regionText() -> String {
                var s = committedStr
                if !draft.isEmpty {
                    s += (s.isEmpty ? sep0() : "\n\n") + draft
                }
                return s
            }
            func persist() {
                var new = immutableDisk + regionText()
                if !new.hasSuffix("\n") { new += "\n" }
                try? new.write(toFile: currentPath, atomically: true, encoding: .utf8)
                immutableDisk = new
            }
            w.onMeterRecord = {
                switch voice.state {
                case .idle:
                    // anchor on the EDITOR's DISPLAY string so the tail math
                    // (immLen + committedLen) matches the text storage exactly
                    // — currentEditorText is markdown-serialized and would
                    // misalign offsets (and flatten images) on image notes
                    immutable = w.editorText
                    immutableDisk = (try? String(contentsOfFile: currentPath,
                                                 encoding: .utf8)) ?? ""
                    committedStr = ""
                    draft = ""
                    dbg("record start immutable=\(immLen()) disk=\(immutableDisk.count)")
                    liveWrite = Timer.scheduledTimer(withTimeInterval: 1.5,
                                                     repeats: true) { _ in
                        guard !draft.isEmpty else { return }
                        persist()
                    }
                    voice.start()
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
            voice.onStateChange = { [weak w] _ in
                guard let w else { return }
                w.recordingState = voice.state.rawValue
                w.recordingElapsed = voice.elapsed
            }
            voice.onLevel = { [weak w] level in
                guard let w else { return }
                w.recordingLevel = level
                w.recordingElapsed = voice.elapsed
            }
            // live draft: rebuild ONLY the region after the immutable prefix
            voice.onPartial = { [weak w] text in
                guard let w, !text.isEmpty else { return }
                draft = text
                w.replaceTail(from: immLen() + committedLen(), with: regionText())
                // follow the draft: the dictated text lives at the end of the
                // note, so pin the view to the bottom as it streams in
                w.scrollEditorToEnd()
            }
            // finalized batch: grow committedStr, persist, drop the draft
            voice.onBatch = { [weak self, weak w] text in
                guard let self, let w else { return }
                let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty {
                    committedStr += (committedStr.isEmpty ? sep0() : "\n\n") + trimmed
                    draft = ""
                    persist()
                    w.replaceTail(from: immLen(), with: regionText())
                    w.scrollEditorToEnd()
                    dbg("batch +\(trimmed.count) committed=\(committedLen())")
                    if voice.state == .transcribing {
                        liveWrite?.invalidate()
                        liveWrite = nil
                        // fold the session into the immutable base (display
                        // string; the file already holds the persisted text)
                        immutable = w.editorText
                        immutableDisk = (try? String(contentsOfFile: currentPath,
                                                     encoding: .utf8)) ?? immutableDisk
                        committedStr = ""
                        voice.resetSession()
                    }
                } else if voice.state == .transcribing {
                    liveWrite?.invalidate()
                    liveWrite = nil
                    draft = ""
                    w.replaceTail(from: immLen() + committedLen(), with: regionText())
                    voice.resetSession()
                    self.log("voice '\(cmd.name)': no speech detected")
                }
            }
            voice.onError = { [weak self, weak w] err in
                self?.log("voice '\(cmd.name)': \(err)")
                dbg("error: \(err)")
                // visible feedback WITHOUT polluting the note: the footer
                // line shows the problem; the note keeps only dictated text
                if let w {
                    w.tabFooterText = "⚠️ \(err)"
                    w.scrollEditorToEnd()
                }
                voice.resetSession()
            }
            w.onHideVoiceStop = {
                liveWrite?.invalidate()
                liveWrite = nil
                voice.stop()
            }
        }
        // save current text — but NEVER resurrect a deleted note: if the file
        // vanished, park the text in a fresh default.md next to it and swap
        // the tab (Cmd+S / close / tab-change all go through here)
        let commitSave: (String) -> Void = { [weak self] text in
            guard let self else { return }
            // previews (PDF/image) are read-only — never write text back
            guard !noteIsPreview(currentPath) else { return }
            if FileManager.default.fileExists(atPath: currentPath) {
                self.saveNote(text, to: currentPath, cmd: cmd)
                lastSynced = text
                lastMtime = mtime(of: currentPath)
                w.tabFooterText = lastWriteLabel(currentPath)
                return
            }
            let deadPath = currentPath
            let fallback = noteDir(deadPath) + "/default.md"
            try? FileManager.default.createDirectory(atPath: noteDir(fallback),
                                                     withIntermediateDirectories: true)
            if !FileManager.default.fileExists(atPath: fallback) {
                FileManager.default.createFile(atPath: fallback, contents: nil)
            }
            self.log("note '\(cmd.name)': \(deadPath) deleted — text parked in \(fallback)")
            self.saveNote(text, to: fallback, cmd: cmd)
            if let idx = paths.firstIndex(of: deadPath) {
                paths[idx] = fallback
                titles[idx] = URL(fileURLWithPath: fallback).lastPathComponent
            } else {
                paths.append(fallback)
                titles.append(URL(fileURLWithPath: fallback).lastPathComponent)
            }
            w.tabTitles = titles
            self.removeNotePathFromConfig(deadPath, section: cmd.name)
            self.addNotePathToConfig(fallback, section: cmd.name)
            currentPath = fallback
            lastSynced = text
            lastMtime = mtime(of: fallback)
            w.tabFooterText = lastWriteLabel(fallback)
        }
        w.onEditorCommit = commitSave
        w.onEditorClose = commitSave
        w.onHide = { [weak self] restore in
            guard let self else { return }
            // if the color panel is open on this window, don't leave it
            // floating once the notes window hides
            self.dismissPickerIfOpen(for: w)
            // Persistent singleton note window: keep the PopupWindow (and its
            // embedded terminal session) alive — just hide the panel. The next
            // Hyper+N re-shows the SAME instance instead of spawning a fresh
            // terminal. Focus is still handed back to the window we came from.
            // The note watcher keeps running; its closure self-guards on
            // w.isShown while hidden, so nothing needs restarting on re-show.
            self.restoreFocus(restore)
        }
        // poll EVERY tab's note for external writes (1s): reload the active note
        // when it changes on disk (unless there are unsaved edits) and watch
        // for notes being DELETED. A deleted note is never resurrected — its
        // tab becomes default.md in the same directory, and the active note's
        // on-screen text is parked into that default.md so nothing is lost.
        let t = Timer(timeInterval: noteWatchInterval, repeats: true) { [weak self, weak w] _ in
            guard let self, let w, w.isShown else { return }
            var dirty = false
            var i = 0
            while i < paths.count {
                let p = paths[i]
                if FileManager.default.fileExists(atPath: p) { i += 1; continue }
                // note deleted on disk
                let fallback = noteDir(p) + "/default.md"
                try? FileManager.default.createDirectory(atPath: noteDir(fallback),
                                                         withIntermediateDirectories: true)
                if !FileManager.default.fileExists(atPath: fallback) {
                    FileManager.default.createFile(atPath: fallback, contents: nil)
                }
                if p == currentPath {
                    let text = noteIsPreview(p) ? "" : w.currentEditorText
                    self.log("note '\(cmd.name)': \(p) deleted on disk — text parked in \(fallback)")
                    self.saveNote(text, to: fallback, cmd: cmd)
                    currentPath = fallback
                    lastSynced = text
                    lastMtime = mtime(of: fallback)
                    w.tabFooterText = lastWriteLabel(fallback)
                    w.setEditorMarkdown(text, baseDir: noteDir(fallback))
                    w.imageBaseDir = noteDir(fallback)
                } else {
                    self.log("note '\(cmd.name)': \(p) deleted on disk — tab now default.md")
                }
                self.removeNotePathFromConfig(p, section: cmd.name)
                if let existing = paths.firstIndex(of: fallback), existing != i {
                    // default.md already a tab — drop the dead entry instead
                    paths.remove(at: i)
                    titles.remove(at: i)
                } else {
                    paths[i] = fallback
                    titles[i] = URL(fileURLWithPath: fallback).lastPathComponent
                    self.addNotePathToConfig(fallback, section: cmd.name)
                    i += 1
                }
                dirty = true
            }
            // NEW notes in a configured directory show up as tabs on their
            // own — no config edit needed (directory entries cover them).
            // Dismissed notes (✕) are never re-added by the sync.
            for p in expandPaths(cmd.paths, extensions: ["md"])
            where FileManager.default.fileExists(atPath: p) && !paths.contains(p)
                && !DismissedNotes.contains(p) {
                paths.append(p)
                titles.append(URL(fileURLWithPath: p).lastPathComponent)
                self.log("note '\(cmd.name)': new note detected — added tab \(p)")
                dirty = true
            }
            if dirty {
                w.tabTitles = titles
                // keep the strip highlight on the active note
                if let active = paths.firstIndex(of: currentPath), active != w.selectedTab {
                    w.selectedTab = active
                }
            }
            // active-note external-write reload (skipped for read-only previews)
            if let mt = mtime(of: currentPath), !noteIsPreview(currentPath) {
                if let last = lastMtime, mt != last {
                    if w.currentEditorText == lastSynced {
                        let newText = (try? String(contentsOfFile: currentPath, encoding: .utf8)) ?? ""
                        if newText != lastSynced {
                            w.setEditorMarkdown(newText, baseDir: noteDir(currentPath))
                            lastSynced = newText
                            w.tabFooterText = lastWriteLabel(currentPath)
                            self.log("note '\(cmd.name)': reloaded \(currentPath) after external write")
                        }
                    } else {
                        self.log("note '\(cmd.name)': external change to \(currentPath) ignored (unsaved edits)")
                    }
                }
                lastMtime = mt
            }
        }
        RunLoop.main.add(t, forMode: .common)
        watcher = t
        // embedded file browser drawer (header "▤" toggles it): starts in the
        // note directory, favorites shared with the floating "files" window
        let favs = fileBrowserFavoritesConfig()
        let fb = PopupFileBrowser(config: cfg, startDir: noteDir(currentPath),
                                  staticFavorites: favs.staticFavs,
                                  zoxideFavorites: zoxideTopDirs(favs.zoxideTop))
        fb.onOpen = { [weak self] path in
            self?.log("note '\(cmd.name)': browser opened \(path)")
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
        fb.onCopyPath = { [weak self] p in
            self?.copy(p, "path: \(p)")
        }
        fb.onCopyDir = { [weak self] dir in
            self?.copy(dir, "directory path: \(dir)")
        }
        fb.onStatus = { [weak self] s in
            if !s.isEmpty { self?.log("note '\(cmd.name)': \(s)") }
        }
        // file-browser right-click "Open in Notes": open the row as a note tab
        w.onFileBrowserOpenInNotes = { [weak self] p in
            self?.openNoteFile(p)
        }
        w.installFileBrowser(fb, drawer: true)
        // mirror the post-install drawer state onto the header buttons
        // (browser is the default pane, so it's on and the terminal is off)
        w.setHeaderButtonOn(10, w.terminalShown)
        w.setHeaderButtonOn(20, w.fileBrowserShown)
        subWindows.append(w)
        w.show()
    }

    private func saveNote(_ text: String, to path: String, cmd: CommandSpec) {
        do {
            try text.write(toFile: path, atomically: true, encoding: .utf8)
        } catch {
            log("note '\(cmd.name)': save failed: \(error)")
        }
    }

    // keep commands.conf in sync: append a newly created note to the [notes]
    // section's paths= line, using the tilde form for paths under $HOME
    private func addNotePathToConfig(_ path: String, section: String) {
        let confPath = settings.commandsConfPath
        guard let content = try? String(contentsOfFile: confPath, encoding: .utf8) else {
            log("commands.conf: cannot read \(confPath)")
            return
        }
        let home = NSHomeDirectory()
        let display = path.hasPrefix(home + "/")
            ? "~" + path.dropFirst(home.count)
            : path
        var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let target = "[" + section + "]"
        // is this note already listed? (e.g. + re-created with an existing name)
        // — compare EXPANDED forms so ~/notes/x.md == /Users/me/notes/x.md
        let isListed = { (value: String) -> Bool in
            value.split(separator: ",").contains { entry in
                let e = entry.trimmingCharacters(in: .whitespaces)
                return (e as NSString).expandingTildeInPath == path
            }
        }
        var inSection = false
        for (i, line) in lines.enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") && trimmed.hasSuffix("]") {
                inSection = trimmed == target
                continue
            }
            guard inSection, let eq = trimmed.firstIndex(of: "=") else { continue }
            let key = String(trimmed[..<eq]).trimmingCharacters(in: .whitespaces)
            if key == "paths" {
                let value = String(trimmed[trimmed.index(after: eq)...])
                    .trimmingCharacters(in: .whitespaces)
                if isListed(value) {
                    log("commands.conf: \(display) already listed — no change")
                    return
                }
                lines[i] = line + ", " + display
            } else if key == "path" {
                let value = String(trimmed[trimmed.index(after: eq)...])
                    .trimmingCharacters(in: .whitespaces)
                if isListed(value) {
                    log("commands.conf: \(display) already listed — no change")
                    return
                }
                lines[i] = "paths = " + value + ", " + display
            } else {
                continue
            }
            try? (lines.joined(separator: "\n"))
                .write(toFile: confPath, atomically: true, encoding: .utf8)
            log("commands.conf: added note \(display)")
            return
        }
        log("commands.conf: no [\(section)] section to update")
    }

    // drop a deleted note from commands.conf so it never gets listed again
    // (paths= entries that no longer exist on disk are removed)
    private func removeNotePathFromConfig(_ path: String, section: String) {
        let confPath = settings.commandsConfPath
        guard let content = try? String(contentsOfFile: confPath, encoding: .utf8) else {
            log("commands.conf: cannot read \(confPath)")
            return
        }
        let home = NSHomeDirectory()
        let display = path.hasPrefix(home + "/")
            ? "~" + path.dropFirst(home.count)
            : path
        var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let target = "[" + section + "]"
        var inSection = false
        for (i, line) in lines.enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") && trimmed.hasSuffix("]") {
                inSection = trimmed == target
                continue
            }
            guard inSection, let eq = trimmed.firstIndex(of: "=") else { continue }
            let key = String(trimmed[..<eq]).trimmingCharacters(in: .whitespaces)
            guard key == "paths" || key == "path" else { continue }
            let value = String(trimmed[trimmed.index(after: eq)...])
                .trimmingCharacters(in: .whitespaces)
            let kept = value.split(separator: ",").compactMap { entry -> String? in
                let e = String(entry).trimmingCharacters(in: .whitespaces)
                return (e as NSString).expandingTildeInPath == path ? nil : e
            }
            guard kept.count != value.split(separator: ",").count else { continue }
            if kept.isEmpty {
                lines.remove(at: i)
            } else {
                lines[i] = "paths = " + kept.joined(separator: ", ")
            }
            try? (lines.joined(separator: "\n"))
                .write(toFile: confPath, atomically: true, encoding: .utf8)
            log("commands.conf: removed \(display) from [\(section)]")
            return
        }
        log("commands.conf: no [\(section)] section to update")
    }

    // MARK: Interactive color picker (header paint-brush button)

    // The paint-brush header button: a SHORT menu of the window's background
    // roles; picking one opens the shared NSColorPanel for that role. Dragging
    // PREVIEWS live in this window only; nothing is written until "Apply".
    // Pressing the panel's close "x" (or Esc) reverts to the original color.
    private func presentThemeRoleMenu(for w: PopupWindow,
                                      roles: [PopupWindow.ThemeRole],
                                      section: String) {
        let menu = NSMenu(title: "Pick a color for…")
        for role in roles {
            let item = NSMenuItem(title: role.label,
                                  action: #selector(pickThemeRole(_:)),
                                  keyEquivalent: "")
            item.target = self
            item.representedObject = role
            menu.addItem(item)
        }
        menu.addItem(.separator())
        let reset = NSMenuItem(title: "Reset to system defaults",
                               action: #selector(resetThemeColors(_:)),
                               keyEquivalent: "")
        reset.target = self
        menu.addItem(reset)
        pickerWindow = w
        pickerSection = section
        // pop under the paint-brush button (fall back to the mouse position)
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

    // "Reset to system defaults": drop every color override this window
    // carries in commands.conf and live-restore the app-wide ([theme])
    // defaults — browser, terminal, notepad and header all go back.
    @objc private func resetThemeColors(_ sender: Any?) {
        guard let w = pickerWindow else { return }
        let section = pickerSection
        removeColorKeysFromConfig(section: section)
        // the [theme] app-wide defaults (or the built-in fallbacks)
        let base = PopupConfig(name: "")
        let browserDefault = THEME_BROWSER ?? base.fileBrowserBackground
        let terminalDefault = THEME_TERMINAL ?? base.terminalBackground
        let notepadDefault = BAR.withAlphaComponent(base.tintAlpha)
        w.setThemeColor(browserDefault, for: .browser)
        w.setThemeColor(terminalDefault, for: .terminal)
        w.setThemeColor(notepadDefault, for: .notepad)
        w.setThemeColor(headerBlueSilver, for: .header)
        NSColorPanel.shared.orderOut(nil)
        log("theme reset for [\(section)] — back to system defaults")
    }

    private func startColorPicker(for w: PopupWindow, role: PopupWindow.ThemeRole) {
        pickerWindow = w
        pickerRole = role
        pickerHex = ""
        pickerOriginal = w.themeColor(role)
        pickerCommitted = false
        pickerSawVisible = false
        let panel = NSColorPanel.shared
        panel.color = pickerOriginal ?? .clear
        // opacity slider ON — the alpha you pick is the surface's opacity
        panel.showsAlpha = true
        panel.isContinuous = true
        panel.setTarget(self)
        panel.setAction(#selector(panelColorChanged(_:)))
        // accessory row: Apply commits + saves, Cancel reverts + closes. The
        // panel's own close "x" / Esc are cancel too (see watchdog below).
        let applyButton = NSButton(title: "Apply", target: self,
                                   action: #selector(applyPickerColor(_:)))
        applyButton.keyEquivalent = "\r"
        applyButton.bezelStyle = .rounded
        let cancelButton = NSButton(title: "Cancel", target: self,
                                    action: #selector(cancelPickerColor(_:)))
        cancelButton.bezelStyle = .rounded
        let acc = NSView(frame: NSRect(x: 0, y: 0, width: 180, height: 32))
        applyButton.frame = NSRect(x: 0, y: 2, width: 82, height: 26)
        cancelButton.frame = NSRect(x: 92, y: 2, width: 82, height: 26)
        acc.addSubview(applyButton)
        acc.addSubview(cancelButton)
        panel.accessoryView = acc
        // backup cancel path (in case the watchdog hasn't ticked yet)
        if let o = pickerPanelObserver {
            NotificationCenter.default.removeObserver(o)
        }
        pickerPanelObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification, object: panel, queue: .main
        ) { [weak self] _ in
            self?.revertPickerIfCancelled()
        }
        // the color panel dismisses with orderOut (not close) on "x"/Esc, so
        // willClose alone can't catch a cancel — watch for the panel going
        // invisible without Apply having been pressed.
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
        log("color picker opened for [\(pickerSection)] \(role.rawValue)")
    }

    @objc private func panelColorChanged(_ sender: Any?) {
        guard let w = pickerWindow, let role = pickerRole else { return }
        // live preview ONLY — nothing is written until Apply
        let c = NSColorPanel.shared.color
        w.setThemeColor(c, for: role)
        pickerHex = hexString(c)
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

    // The color panel must never linger once the window it edits is hidden —
    // close it and revert any un-committed preview.
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
        guard let w = pickerWindow, let role = pickerRole else { return }
        let hex = pickerHex.isEmpty ? hexString(w.themeColor(role)) : pickerHex
        guard !hex.isEmpty, !pickerSection.isEmpty else { return }
        // per-window override keys — the pick only affects THIS window
        let key: String
        switch role {
        case .browser: key = "browser-background"
        case .terminal: key = "terminal-background"
        case .notepad: key = "background-color"
        case .header: key = "header-color"
        }
        updateColorKeyInConfig(hex, key: key, section: pickerSection)
        log("commands.conf [\(pickerSection)]: \(key) -> #\(hex)")
    }

    private func hexString(_ c: NSColor) -> String {
        let cc = c.usingColorSpace(.sRGB) ?? c
        let r = Int(round(cc.redComponent * 255))
        let g = Int(round(cc.greenComponent * 255))
        let b = Int(round(cc.blueComponent * 255))
        let a = Int(round(cc.alphaComponent * 255))
        // an explicit non-opaque alpha is stored as AARRGGBB so the opacity
        // slider's pick survives a restart
        return a < 255
            ? String(format: "%02X%02X%02X%02X", a, r, g, b)
            : String(format: "%02X%02X%02X", r, g, b)
    }

    // rewrite (or insert) a KEY = VALUE line in a [section] of commands.conf,
    // preserving comments/order; the app reads colors at startup so a restart
    // picks up the persisted pick
    private func updateColorKeyInConfig(_ value: String, key: String, section: String) {
        let confPath = settings.commandsConfPath
        guard let content = try? String(contentsOfFile: confPath, encoding: .utf8) else {
            log("commands.conf: cannot read \(confPath)")
            return
        }
        var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let target = "[" + section + "]"
        var inSection = false
        for (i, line) in lines.enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") && trimmed.hasSuffix("]") {
                inSection = trimmed == target
                continue
            }
            guard inSection, let eq = trimmed.firstIndex(of: "=") else { continue }
            let k = String(trimmed[..<eq]).trimmingCharacters(in: .whitespaces)
            if k == key {
                lines[i] = key + " = " + value
                try? (lines.joined(separator: "\n"))
                    .write(toFile: confPath, atomically: true, encoding: .utf8)
                return
            }
        }
        // no existing key: insert right after the section header (or append a
        // fresh section when the section doesn't exist yet)
        var insertion: Int
        if let idx = lines.firstIndex(where: {
            $0.trimmingCharacters(in: .whitespaces) == target
        }) {
            insertion = idx + 1
        } else {
            lines.append("")
            lines.append(target)
            insertion = lines.count
        }
        lines.insert(key + " = " + value, at: insertion)
        try? (lines.joined(separator: "\n"))
            .write(toFile: confPath, atomically: true, encoding: .utf8)
    }

    // drop every color-override key from a [section] so the [theme] defaults
    // apply again ("Reset to system defaults" in the picker menu)
    private func removeColorKeysFromConfig(section: String) {
        let confPath = settings.commandsConfPath
        guard let content = try? String(contentsOfFile: confPath, encoding: .utf8) else {
            log("commands.conf: cannot read \(confPath)")
            return
        }
        let keys: Set<String> = ["header-color", "background-color",
                                 "browser-background", "terminal-background",
                                 "tint-alpha"]
        let target = "[" + section + "]"
        var inSection = false
        var changed = false
        var out: [String] = []
        out.reserveCapacity(64)
        for line in content.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") && trimmed.hasSuffix("]") {
                inSection = trimmed == target
            } else if inSection, let eq = trimmed.firstIndex(of: "=") {
                let k = String(trimmed[..<eq]).trimmingCharacters(in: .whitespaces)
                if keys.contains(k) {
                    changed = true
                    continue
                }
            }
            out.append(String(line))
        }
        guard changed else {
            log("commands.conf [\(section)]: no color overrides to reset")
            return
        }
        try? out.joined(separator: "\n")
            .write(toFile: confPath, atomically: true, encoding: .utf8)
        log("commands.conf [\(section)]: reset color overrides to defaults")
    }

    private func openListWindow(_ cmd: CommandSpec,
                                restoreWID: String?, restorePID: pid_t?) {
        guard !cmd.sources.isEmpty else {
            log("list '\(cmd.name)': no source configured")
            return
        }
        // each source: { path, rows }
        // a source may be a single file OR a directory — a directory expands
        // to all matching files (sorted), so adding a file to a folder needs
        // no commands.conf edit
        var tabs: [(path: String, items: [FieldRow])] =
            expandPaths(cmd.sources, extensions: ["json", "tsv"]).map { path in
                return (path, loadListItems(path, cmd: cmd))
            }
        var currentTab = 0
        // filter dimensions that actually exist in the current tab (parallel
        // to the window's filterValues/selections)
        var activeDims: [String] = []
        func currentItems() -> [FieldRow] { tabs[currentTab].items }

        // refresh the dropdown dimensions for the current tab; dims with no
        // values in this tab disappear from the bar entirely
        func applyFilterData() {
            let fd = filterData(currentItems())
            activeDims = fd.dims
            w.filterLabels = fd.dims
            w.filterValues = fd.values
            w.filterValueLabels = fd.labels
            w.filterSelections = Array(repeating: 0, count: fd.dims.count)
            w.growWidthToContent()
        }

        // unique dropdown values per filter dimension for a tab ("All" first), plus
// display titles: the release dropdown shows "name (date)" (from the parallel
// releaseLabel field) while still MATCHING on the raw release name — filter
// logic uses `values`, labels are display-only.
func filterData(_ items: [FieldRow]) -> (dims: [String], values: [[String]], labels: [[String]]) {
    var dims: [String] = []
    var values: [[String]] = []
    var labels: [[String]] = []
    for key in cmd.filters {
        var seen = Set<String>()
        var vals = items.compactMap { $0.fields[key] }
            .filter { !$0.isEmpty && seen.insert($0).inserted }
        vals.sort()
        var labelByValue: [String: String] = [:]
        if key == "release" {
            for row in items {
                let names = (row.fields["release"] ?? "")
                    .split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                let ls = (row.fields["releaseLabel"] ?? "")
                    .split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                for (i, n) in names.enumerated() where !n.isEmpty && labelByValue[n] == nil {
                    labelByValue[n] = i < ls.count && !ls[i].isEmpty ? ls[i] : n
                }
            }
        }
        // a dimension with no values in THIS tab is hidden entirely — showing
        // "releaseStatus: All" over all.json (which has no releaseStatus) was
        // a dead control that only added noise to the bar
        guard !vals.isEmpty else { continue }
        dims.append(key)
        values.append(["All"] + vals)
        labels.append(["All"] + vals.map { labelByValue[$0] ?? $0 })
    }
    return (dims, values, labels)
}

        var cfg = PopupConfig(name: cmd.windowName)
        cfg.enableToggle = false
        cfg.enableResize = cmd.resize
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.wrapContent = true
        cfg.showSearchBar = true
        cfg.dragHeader = true
        cfg.tabs = tabs.count > 1
        cfg.scrollableRows = true
        cfg.dynamicHeight = false
        cfg.clickToSelect = true
        cfg.wrapNavigation = false
        cfg.highlightMatches = true
        cfg.filters = !cmd.filters.isEmpty
        cfg.selectableRows = cmd.checkbox ?? !cmd.copyFields.isEmpty
        cfg.bodyMaxLines = cmd.bodyLines > 0 ? cmd.bodyLines : 5
        cfg.height = cmd.height > 0 ? cmd.height : defaultListSize.height
        cfg.width = cmd.width > 0 ? cmd.width : defaultListSize.width
        // slim header (same height as the jira detail window): no title pill,
        // bluey-silver strip, app glyph far left with the meta line beside it
        cfg.headerHeight = 30
        cfg.titlePill = false
        cfg.headerColor = cmd.headerColor ?? headerBlueSilver
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        if cmd.searchWidth > 0 { cfg.searchWidthFraction = cmd.searchWidth }
        if cmd.maxStretch > 0 { cfg.maxRowStretch = cmd.maxStretch }
        cfg.fontName = cmd.font
        let w = PopupWindow(config: cfg)
        // empty `title` in commands.conf = no header label (icon still shows)
        w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
        w.headerIcon = jiraAppIcon
        // row copy: ticked rows (or every row) serialized as TSV lines of the
        // configured copy-fields; the framework owns checkboxes + button +
        // pasteboard, this closure is the only list-specific part
        let copyKeys = cmd.copyFields
        w.onCopyRows = { picked in
            picked.compactMap { $0 as? FieldRow }
                .filter { !$0.loadMore }
                .map { row in
                    copyKeys.map { row.fields[$0] ?? "" }.joined(separator: "\t")
                }
                .joined(separator: "\n")
        }
        // clicking the drag header copies the active tab's source path
        w.onChromeHeaderClick = { [weak self] in
            guard let self, tabs.indices.contains(currentTab) else { return }
            self.copy(tabs[currentTab].path, "source path: \(tabs[currentTab].path)")
        }
        // header "config" button: copy the commands.conf path
        w.onChromeConfigClick = { [weak self] in
            self?.copy(settings.commandsConfPath, "config path: \(settings.commandsConfPath)")
        }
        let cap = cmd.maxRows > 0 ? cmd.maxRows : Int.max
        var visibleOffset = 0
        var reloadWatcher: Timer?
        // header "copy … path" button follows the ACTIVE tab; refreshed on
        // every tab change and every on-disk reload, not just once at open
        func refreshPathLabel() {
            guard tabs.indices.contains(currentTab) else { return }
            w.copyPathButtonLabel =
                "copy \(URL(fileURLWithPath: tabs[currentTab].path).lastPathComponent) path"
        }

        // combined filter: search (fuzzy) + dropdown selections
        func filteredRows(query: String) -> [FieldRow] {
            let byQuery = PopupFuzzy.filter(currentItems(), query: query) { $0.searchText }
            let result: [FieldRow]
            if activeDims.isEmpty {
                result = Array(byQuery.prefix(cap))
            } else {
                let opts = w.filterValues
                result = Array(byQuery.filter { row in
                    for (i, sel) in w.filterSelections.enumerated() where sel > 0 {
                        guard i < activeDims.count, opts.indices.contains(i),
                              opts[i].indices.contains(sel) else { continue }
                        if row.fields[activeDims[i]] != opts[i][sel] { return false }
                    }
                    return true
                }.prefix(cap))
            }
            // paging: page-size > 0 keeps huge lists snappy while browsing; a
            // "load more" row at the bottom reveals the next page on Enter or
            // click. Searching is cheap (capped search text) and rendering a
            // narrowed result is fine, so an ACTIVE QUERY shows every match —
            // an item beyond the current page still renders when found.
            var paged = result
            if cmd.pageSize > 0, result.count > cmd.pageSize, query.isEmpty {
                paged = Array(result.prefix(visibleOffset + cmd.pageSize))
                if paged.count < result.count {
                    let remaining = result.count - paged.count
                    paged.append(FieldRow(title: "load \(remaining) more…",
                                          content: nil, trailing: nil, detail: nil,
                                          body: nil, searchText: "",
                                          fields: ["__loadmore": "1"]))
                }
            }
            // live header count: current items in the list (updates with search)
            w.itemCount = paged.count == 1 ? "1 item" : "\(paged.count) items"
            return paged
        }

        applyFilterData()
        w.tabTitles = tabs.map { URL(fileURLWithPath: $0.path).lastPathComponent }
        w.onFilter = { [weak self] query in
            guard self != nil else { return [] }
            visibleOffset = 0
            return filteredRows(query: query)
        }
        w.onFilterChange = { [weak self] _ in
            guard self != nil else { return }
            visibleOffset = 0
            w.setRows(filteredRows(query: w.currentQuery))
        }
        w.onTabChange = { [weak self] index in
            guard let self, index < tabs.count, index != currentTab else { return }
            currentTab = index
            visibleOffset = 0
            w.clearInput()
            applyFilterData()
            w.setRows(filteredRows(query: ""))
            w.tabFooterText = lastWriteLabel(tabs[currentTab].path)
            refreshPathLabel()
            self.log("list '\(cmd.name)': tab -> \(tabs[index].path)")
        }
        // clicking the ACTIVE tab copies that source's absolute path
        w.onTabClick = { [weak self] index in
            guard let self, index == w.selectedTab, index < tabs.count else { return }
            self.copy(tabs[index].path, "source path: \(tabs[index].path)")
        }
        w.onAccept = { [weak self] row in
            guard let self else { return }
            if row.loadMore {
                visibleOffset += cmd.pageSize
                w.setRows(filteredRows(query: w.currentQuery))
                self.log("list '\(cmd.name)': load more -> offset \(visibleOffset)")
                return
            }
            self.log("list '\(cmd.name)': picked '\(row.title)'")
            w.hide(restore: true)
        }
        w.onRowClick = { [weak self] index in
            guard let self, index >= 0, index < w.rows.count else { return }
            if w.rows[index].loadMore {
                visibleOffset += cmd.pageSize
                w.setRows(filteredRows(query: w.currentQuery))
                self.log("list '\(cmd.name)': load more (click) -> offset \(visibleOffset)")
            }
        }
        // double-click a row -> "more details" (no per-row action label)
        w.onRowDoubleClick = { [weak self] index in
            guard let self, index >= 0, index < w.rows.count,
                  let row = w.rows[index] as? FieldRow, !row.loadMore else { return }
            self.showDetail(row, cmd: cmd)
        }
        w.onEscape = { w.hide(restore: true) }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            reloadWatcher?.invalidate()
            reloadWatcher = nil
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: restoreWID, restorePID: restorePID)
        }
        // reload a tab when its source file changes on disk (e.g. the poll
        // wrote fresh json) so an open window never shows stale rows
        var tabMtimes = tabs.map { mtime(of: $0.path) }
        let watcher = Timer(timeInterval: listWatchInterval, repeats: true) { [weak self, weak w] _ in
            guard let self, let w, w.isShown else { return }
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
                tabs[i].items = loadListItems(tabs[i].path, cmd: cmd)
                self.log("list '\(cmd.name)': reloaded \(tabs[i].path) after external write")
            }
            w.tabTitles = tabs.map { URL(fileURLWithPath: $0.path).lastPathComponent }
            refreshPathLabel()
            // only re-filter when the tab on screen is one that changed
            guard changed.contains(currentTab) else { return }
            applyFilterData()
            visibleOffset = 0
            // keep the scroll position: a background json refresh must not
            // yank the list back to the top while the user reads mid-list
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            w.tabFooterText = lastWriteLabel(tabs[currentTab].path)
        }
        RunLoop.main.add(watcher, forMode: .common)
        reloadWatcher = watcher
        w.copyConfigButtonLabel = "copy config path"
        refreshPathLabel()
        subWindows.append(w)
        w.tabFooterText = lastWriteLabel(tabs[currentTab].path)
        w.show()
    }

    // Static favorite dirs + zoxide top-N for the file browser, taken from the
    // [files] command section so the notes drawer and the floating window share
    // the same config. Zoxide top-N requires `zoxide` on PATH.
    private func fileBrowserFavoritesConfig() -> (staticFavs: [String], zoxideTop: Int) {
        let cmd = commands.first(where: { $0.kind == .files })
        return (cmd?.favorites ?? [], cmd?.zoxideTop ?? 0)
    }

    // zoxide query -l lists every directory ranked by frecency; take the top N
    // that still exist. Silent if zoxide is not installed.
    private func zoxideTopDirs(_ n: Int) -> [String] {
        guard n > 0 else { return [] }
        var bin: String? = nil
        for cand in ["/opt/homebrew/bin/zoxide", "/usr/local/bin/zoxide"] {
            if FileManager.default.isExecutableFile(atPath: cand) { bin = cand; break }
        }
        guard let bin else { return [] }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: bin)
        p.arguments = ["query", "-l"]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = Pipe()
        do { try p.run() } catch { return [] }
        p.waitUntilExit()
        let data = (try? out.fileHandleForReading.readToEnd()) ?? Data()
        return String(data: data, encoding: .utf8)?
            .split(separator: "\n")
            .map { String($0).trimmingCharacters(in: .whitespaces) }
            .filter { FileManager.default.fileExists(atPath: $0) }
            .prefix(n)
            .map { $0 } ?? []
    }

    // Read-only file browser ("files" commands): a keyboard-driven directory
    // listing in the searchable list window. Type to fuzzy-filter the current
    // directory; Enter on a file opens it with its default app; Enter on a
    // directory (or the ".." row) navigates into it. No rename/delete/create.
    private func openFilesWindow(_ cmd: CommandSpec,
                                 restoreWID: String?, restorePID: pid_t?) {
        let root = ((cmd.root ?? "~") as NSString).expandingTildeInPath
        guard FileManager.default.fileExists(atPath: root) else {
            log("files '\(cmd.name)': root \(root) does not exist")
            return
        }
        var cfg = PopupConfig(name: cmd.windowName)
        cfg.enableToggle = false
        cfg.enableResize = cmd.resize
        cfg.enableDrag = cmd.drag
        cfg.sticky = cmd.sticky
        cfg.enableNavigation = false   // the browser owns up/down/return
        cfg.enableSearch = false       // the browser has its own search field
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
        cfg.headerColor = cmd.headerColor ?? headerBlueSilver
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        if let ta = cmd.tintAlpha { cfg.tintAlpha = ta }
        if let bg = cmd.backgroundColor {
            let cc = bg.usingColorSpace(.sRGB) ?? bg
            // the card hue stays opaque; the color's alpha becomes the card
            // opacity (tintAlpha) so the picker's opacity slider survives
            cfg.colors.background = cc.withAlphaComponent(1)
            cfg.tintAlpha = cc.alphaComponent
        }
        cfg.fontName = cmd.font
        let w = PopupWindow(config: cfg)
        w.headerIcon = NSWorkspace.shared.icon(forFile: root)
        w.chromeHeaderTitle = root
        // paint-brush (id 60): interactive color picker for the explorer
        // panel background (persists to commands.conf on close)
        w.headerButtons = [("\u{F1FC}", 60)]
        w.onHeaderButton = { [weak self, weak w] id in
            guard id == 60, let self, let w else { return }
            self.presentThemeRoleMenu(for: w, roles: [.browser, .header], section: cmd.name)
        }

        let favs = fileBrowserFavoritesConfig()
        let fb = PopupFileBrowser(config: cfg, startDir: root,
                                  staticFavorites: favs.staticFavs,
                                  zoxideFavorites: zoxideTopDirs(favs.zoxideTop))
        fb.onOpen = { [weak self] path in
            self?.log("files '\(cmd.name)': opened \(path)")
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        }
        fb.onCopyPath = { [weak self] p in
            self?.copy(p, "path: \(p)")
        }
        // keep the window title + copy button following the browser's cwd
        fb.onDirChange = { [weak self, weak w] dir in
            guard let self, let w else { return }
            w.chromeHeaderTitle = dir
            w.copyPathButtonLabel = "copy \(URL(fileURLWithPath: dir).lastPathComponent) path"
            w.headerIcon = NSWorkspace.shared.icon(forFile: dir)
            w.growWidthToContent()
        }
        fb.onCopyDir = { [weak self] dir in
            self?.copy(dir, "directory path: \(dir)")
        }
        fb.onStatus = { [weak self] s in
            if !s.isEmpty { self?.log("files '\(cmd.name)': \(s)") }
        }
        // file-browser right-click "Open in Notes": open the row in the notes window
        w.onFileBrowserOpenInNotes = { [weak self] p in
            self?.openNoteFile(p)
        }
        w.installFileBrowser(fb, drawer: false)

        w.copyPathButtonLabel = "copy \(URL(fileURLWithPath: root).lastPathComponent) path"
        w.copyConfigButtonLabel = "copy config path"
        // clicking the drag header copies the current directory's absolute path
        w.onChromeHeaderClick = { [weak fb] in
            fb?.copyDir()
        }
        // header "config" button: copy the commands.conf path
        w.onChromeConfigClick = { [weak self] in
            let canonical = NSHomeDirectory() + "/.config/workspace-switcher/commands.conf"
            let p = FileManager.default.fileExists(atPath: canonical)
                ? canonical : settings.commandsConfPath
            self?.copy(p, "config path: \(p)")
        }
        w.onEscape = { w.hide(restore: true) }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            self.dismissPickerIfOpen(for: w)
            self.unregisterSubWindow(w, restore: restore,
                                     restoreWID: restoreWID, restorePID: restorePID)
        }
        subWindows.append(w)
        w.show()
        // give the browser's list keyboard focus (the window's own hidden
        // search field would otherwise take it)
        DispatchQueue.main.async { [weak w, weak fb] in
            if let w, let fb { w.nativeWindow.makeFirstResponder(fb.listView) }
        }
    }

    // Read one list data file. JSON array of objects preferred (fields looked up
    // by name); TSV lines (key<TAB>title<TAB>status) as fallback when the file
    // has no JSON.
    private func loadListItems(_ path: String, cmd: CommandSpec) -> [FieldRow] {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
              let text = String(data: data, encoding: .utf8) else {
            log("list '\(cmd.name)': cannot read \(path)")
            return []
        }
        let fields = cmd.filter.isEmpty
            ? [cmd.primary, cmd.content, cmd.trailing].compactMap { $0 }
            : cmd.filter
        if let arr = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] {
            return arr.compactMap { d in
                let str = { (k: String?) -> String? in
                    k.flatMap { d[$0] as? String }
                }
                guard let title = str(cmd.primary) else { return nil }
                // search/fuzzy only ever needs a prefix of long fields: cap so
                // 500+ rows with huge descriptions stay fast to filter
                let search = fields.compactMap { capped(str($0), 150) }.joined(separator: " ")
                var raw: [String: String] = [:]
                for (k, v) in d {
                    if let s = v as? String { raw[k] = s }
                }
                // detail may name several fields (comma-separated), joined
                // with " · " on the preview meta line, e.g. assignee,reporter
                let detail = cmd.detail.map { spec -> String? in
                    let vals = spec.split(separator: ",")
                        .map { $0.trimmingCharacters(in: .whitespaces) }
                        .compactMap { d[$0] as? String }
                        .filter { !$0.isEmpty }
                    return vals.isEmpty ? nil : vals.joined(separator: " · ")
                }
                // trailing follows the same rule (e.g. status,priority)
                let trailing = cmd.trailing.map { spec -> String? in
                    let vals = spec.split(separator: ",")
                        .map { $0.trimmingCharacters(in: .whitespaces) }
                        .compactMap { d[$0] as? String }
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

    // hard cap for the content field so long titles never push the detail/
    // trailing fields out of the preview line (0 = no cap)
    private func capped(_ s: String?, _ cap: Int) -> String? {
        guard let s, cap > 0, s.count > cap else { return s }
        return String(s.prefix(cap)) + "…"
    }

    private func handleEscape() {
        if commandMode {
            // command menu is showing: drop back to the workspace view,
            // restoring the previous selection
            commandSelection = popup.selection
            commandMode = false
            popup.selection = workspaceSelection
            popup.clearInput()
            popup.setRows(workspaces.map { WorkspaceRow(ws: $0, iconCache: &iconCache) })
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
            // never block the main thread on aerospace IPC during a close —
            // focus restore is best-effort and runs in the background
            DispatchQueue.global(qos: .userInitiated).async {
                _ = aerospaceCall(["focus", "--window-id", wid])
            }
        }
        savedWID = nil
        savedPID = nil
    }
}

let authDebugPath = NSString(string: "~/.cache/ws-auth-debug").expandingTildeInPath

// MARK: - App

// Menu-bar glyph targets: NSStatusItem actions need an @objc selector, so the
// two toggles live on a tiny target object owned by the delegate.
final class StatusBarTarget: NSObject {
    var onNotes: (() -> Void)?
    var onJira: (() -> Void)?
    var onHealth: (() -> Void)?
    @objc func notes(_ sender: Any?) { onNotes?() }
    @objc func jira(_ sender: Any?) { onJira?() }
    @objc func health(_ sender: Any?) { onHealth?() }
}

// Finder services (right-click a file -> Quick Actions): "Copy Path" copies
// the absolute path(s) to the clipboard; "Open in Notes" opens the file in
// the notes window. Registered as NSApp.servicesProvider; the NSServices in
// Info.plist make Finder's context menu offer them for any file.
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
        // fallback: the legacy Finder pasteboard type is a list of path strings
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
    private var statusItems: [NSStatusItem] = []
    private let statusTarget = StatusBarTarget()
    private var servicesHandler: ServicesHandler?

    init(showOnLaunch: Bool, openCommand: String? = nil) {
        self.showOnLaunch = showOnLaunch
        self.openCommand = openCommand
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        installCrashHandler()
        // diag: log the TCC state the process actually sees + how it was
        // launched (touch ~/.cache/ws-auth-debug to enable)
        if FileManager.default.fileExists(atPath: authDebugPath) {
            let mic = AVCaptureDevice.authorizationStatus(for: .audio).rawValue
            let speech = SFSpeechRecognizer.authorizationStatus().rawValue
            let line = "auth-debug: mic=\(mic) speech=\(speech) bundle=\(Bundle.main.bundleIdentifier ?? "nil") launch=\(CommandLine.arguments[0])\n"
            let path = NSString(string: "~/.cache/ws-auth.log").expandingTildeInPath
            if let fh = FileHandle(forWritingAtPath: path) {
                fh.seekToEndOfFile()
                fh.write(Data(line.utf8))
                try? fh.close()
            } else {
                FileManager.default.createFile(atPath: path, contents: Data(line.utf8))
            }
        }
        let c = SwitcherController()
        controller = c
        c.start()
        // Finder right-click services ("Copy Path" / "Open in Notes")
        let sh = ServicesHandler(c)
        servicesHandler = sh
        NSApp.servicesProvider = sh
        NSUpdateDynamicServices()
        installStatusItems(c)
        if showOnLaunch {
            c.show()
        }
        if let name = openCommand {
            if name == "notes" {
                c.showNotes()
            } else {
                c.showCommand(name)
            }
        }
    }

    // One glyph per popup app in the macOS menu bar: ticket = jira,
    // notepad = notes. Click toggles that window (same path as the hotkeys).
    // One utility glyph in the menu bar whose menu toggles every connected
    // window (notes / jira / voice / health checks) — cleaner than a glyph per
    // window, and new windows just add a menu item.
    private func installStatusItems(_ c: SwitcherController) {
        statusTarget.onNotes = { [weak c] in c?.toggleNotes() }
        // single source of truth: the [jira] section in commands.conf. No
        // section = no command = no menu entry.
        if c.commands.contains(where: { $0.name == "jira" }) {
            statusTarget.onJira = { [weak c] in c?.toggleCommand("jira") }
        }
        statusTarget.onHealth = { [weak c] in c?.toggleCommand("health-checks") }
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = utilityMenuGlyph
        item.button?.toolTip = "workspace-switcher windows"
        let menu = NSMenu()
        func add(_ title: String, _ img: NSImage?, _ selector: Selector) {
            let mi = NSMenuItem(title: title, action: selector, keyEquivalent: "")
            mi.target = statusTarget
            mi.image = img
            menu.addItem(mi)
        }
        // voice is merged into the notes window (commands.conf `voice = true`),
        // so the dropdown only lists Notes — voice lives in the notes header.
        add("Notes", notesMenuGlyph, #selector(StatusBarTarget.notes(_:)))
        if c.commands.contains(where: { $0.name == "jira" }) {
            add("Jira", jiraMenuGlyph, #selector(StatusBarTarget.jira(_:)))
        }
        add("Health checks", heartMenuGlyph, #selector(StatusBarTarget.health(_:)))
        item.menu = menu
        statusItems.append(item)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}
