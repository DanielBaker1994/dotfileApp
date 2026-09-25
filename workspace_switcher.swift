import AppKit
import Foundation
import Darwin
import AVFoundation
import Speech

// Tiny target object that holds a closure so NSMenuItem actions can use
// Swift closures instead of @objc selectors. The host retains these in
// `menuActionTargets` while the menu is alive.
final class MenuActionTarget: NSObject {
    private let action: () -> Void
    init(action: @escaping () -> Void) { self.action = action }
    @objc func run() { action() }
}

// Retains MenuActionTarget instances while menus are open so closures survive
// past the popUp() call.
var menuActionTargets: [MenuActionTarget] = []

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
    var terminalFontSize: CGFloat = 13
    // Font menu "Install font…" catalog: (label, brew cask, type). Type is
    // one of nerd | mono | sans | serif and picks the submenu group.
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
    // bundle ids of screenshot tools whose capture overlay is an ordinary
    // window: while one is frontmost our floating windows step down so the
    // selection rectangle draws ON TOP of them ([app] screenshot-apps)
    var screenshotApps = ["org.flameshot", "pl.maketheweb.cleanshotx",
                          "cc.ffitch.shottr", "com.skitch.skitch"]
    // global hide behavior: when false, windows only dismiss via Esc (regardless
    // of per-window sticky); when true (default), non-sticky windows hide on focus loss
    var hideOnFocusLoss = true
    // [app] float: popup windows stay above other apps' windows (default
    // false); per window `float` in its section overrides it
    var float = false
    // [app] esc-close: rapid Esc presses that close a notes / files / jira
    // window (default 2; 1 = single Esc, 0 = never). Per window `esc-close`
    // overrides it. The switcher palette always closes on one Esc.
    var escClose = 2
    // [app] copy-toast: pill shown after Cmd+K copies a file browser path
    // ("{}" = the path; empty = no toast)
    var copyToast = "Copied {} to clipboard"
    // [app] terminal-app: app the file browser's `term` command opens when
    // the window has no embedded shell drawer (default Ghostty, else Terminal)
    var terminalApp = ""
    // derived (recomputed whenever the settings change)
    var commandsConfPath: String { binDir + "/" + commandsConfName }
    var focusFilePath: String { popupTmpDir() + focusFileName }
    var jiraIconPath: String { binDir + "/" + jiraIconName }
    var notesIconPath: String { binDir + "/" + notesIconName }
}
var settings = AppSettings()

// MARK: - Tunables (named constants for the numeric magic)

// Theme ▸ presets raise a surface to at least this opacity so the palette
// actually shows over the desktop blur
let presetMinOpacity: CGFloat = 0.6
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
    guard let content = readConfigText() else { return out }
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

// One `columns` entry of a table-mode list: field:Title:width:align:flags.
// The SAME string drives jira_poll.py's API fields= param (jira_config.py
// parse_columns) — keep the two parsers in step.
struct ListColumn {
    var field: String
    var title: String
    var width: CGFloat          // % of the usable row width, 0 = share leftover
    var align: String           // left | right | center
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

    // back to the commands.conf form (after a divider drag). titles: false =
    // `field::w:align` — jira headers come from the field labels instead
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
                         sortable: sortable)
    }
}

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
    var browserBackground: NSColor?  // files: panel background (default silvery blue)
    var backgroundColor: NSColor?  // note/files: window card fill (the notepad)
    var tintAlpha: CGFloat?        // note/files: card opacity override (0-1)
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
    var sticky: Bool          // stay visible when another app takes focus
    var float: Bool? = nil    // stay above other apps' windows (nil = [app] float)
    var label: String? = nil  // palette text for "/" commands (nil = the section name)
    var tabsOpaque: Bool? = nil  // note: solid (never transparent) tabs strip (nil = on)
    // files: browser sort (name|modified|created|size|kind) + asc/desc, the
    // recursive-search cap/excludes and the filter words that open a terminal
    var sort: String? = nil
    var sortDescending: Bool? = nil
    var searchLimit: Int? = nil
    var searchExclude: [String]? = nil
    var terminalWords: [String]? = nil
    let searchWidth: CGFloat  // list: search bar as a fraction of window width
    let maxStretch: CGFloat   // list: cap on per-row stretch when resized big
    let height: CGFloat       // window height in points
    let maxHeight: CGFloat    // cap on the window height (0 = 60% of screen)
    var font: String?         // font family for this window's text
    var fontSize: CGFloat     // note: editor point size (0 = default 13)
    var headerColor: NSColor? // drag-header tint (nil = window background)
    var voice: Bool           // note: record + transcribe button in the header
    let terminal: Bool        // note: embedded shell drawer at the bottom
    let terminalHeight: CGFloat
    let terminalDir: String?  // note: starting directory for the embedded shell
    var terminalBackground: NSColor?  // note: shell drawer background (silvery blue)
    // per-window text palette (Theme ▸ presets); nil = the [theme] colors
    var textColor: NSColor? = nil
    var dimColor: NSColor? = nil
    var highlightColor: NSColor? = nil
    var accentColor: NSColor? = nil        // active tab / chip underline
    var terminalForeground: NSColor? = nil  // shell drawer text (nil = textColor)
    var vimMode: Bool          // note: edit notes in an embedded nvim pane
    var vimBin: String         // note: vim binary path or name (default "nvim")
    var vimInit: String?       // note: init file for the vim pane (nil = bundled)
    var startDrawer: String    // note: drawer open at launch: browser|terminal|none
    var escClose: Int?         // rapid Esc presses that close the window (nil = [app] esc-close; 0 = never)
    var imageRows: Int         // note: screen rows an inline image gets in vim
    let icon: NSImage?        // window header glyph (jira/notes/heart/png)
    let saveDir: String       // prettyprint: where "save file" writes (default /tmp/)
    // list: spreadsheet mode — `table = true` + `columns = field:Title:
    // width%:align:flags, …` (flags filter / sort, e.g. filter+sort);
    // table-sort = field:asc|desc remembers the last clicked header
    var table: Bool = false
    var columns: [ListColumn] = []
    var tableSort: String? = nil

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
         vimMode: Bool = false, vimBin: String = "nvim",
         vimInit: String? = nil, startDrawer: String = "browser",
         fontSize: CGFloat = 0,
         escClose: Int? = nil, imageRows: Int = 10,
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
        self.vimMode = vimMode
        self.vimBin = vimBin
        self.vimInit = vimInit
        self.startDrawer = startDrawer
        self.fontSize = fontSize
        self.escClose = escClose
        self.imageRows = imageRows
        self.icon = icon
        self.saveDir = saveDir
    }
}

func loadCommands() -> [CommandSpec] {
    // app-level settings first — [app] may sit anywhere in the file
    applyAppConfigFromDisk()
    guard let content = readConfigText() else {
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
    // keep the launchd jira-poll agent in lock-step with commands.conf — it
    // must never run unless [jira] says enabled (or poll-when-disabled) = true
    syncJiraLaunchAgent()
    return cmds
}

// [jira] enabled — THE SWITCH for the jira window, the poll agent and the
// menu-bar "Enable Jira"/"Disable Jira" title. Read straight from commands.conf so
// every caller sees the same truth (loadCommands drops disabled sections).
func jiraEnabledInConfig() -> Bool { jiraConfigFlag("enabled") }

// [jira] poll-when-disabled — the "Keep Polling" answer when the user
// disables jira while the poller runs: the launchd agent stays loaded (and
// jira_poll.py keeps publishing) with the window + menu entries hidden.
func jiraBackgroundPollInConfig() -> Bool { jiraConfigFlag("poll-when-disabled") }

// the launchd agent runs whenever either switch says so
func jiraPollActiveInConfig() -> Bool { jiraEnabledInConfig() || jiraBackgroundPollInConfig() }

// boolean key of the [jira] section, false when absent (disabled by default)
func jiraConfigFlag(_ key: String) -> Bool {
    guard let val = jiraConfigValue(key) else { return false }
    return ["true", "yes", "1", "on"].contains(val.lowercased())
}

// raw value of a [jira] key straight from commands.conf (nil when absent)
func jiraConfigValue(_ key: String) -> String? {
    guard let content = readConfigText() else { return nil }
    var inJira = false
    for line in content.split(separator: "\n") {
        let s = line.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("[") && s.hasSuffix("]") {
            inJira = s == "[jira]"
            continue
        }
        guard inJira, !s.hasPrefix("#"), let eq = s.firstIndex(of: "=") else { continue }
        if s[..<eq].trimmingCharacters(in: .whitespaces) == key {
            return s[s.index(after: eq)...].trimmingCharacters(in: .whitespaces)
        }
    }
    return nil
}

// launchctl is a GUI-session domain: the agent is bootstrapped (loaded) only
// when [jira] enabled = true (or poll-when-disabled = true), booted out
// otherwise. Runs at daemon start and
// on every config reload (the menu-bar switch), so the poll literally cannot
// run in the background when jira is disabled. The installed plist is kept
// in sync with the repo template (jira/com.jira.poll.plist, __WS_CONFIG__
// substituted); an already-loaded, unchanged agent is left running.
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
    if let tmpl = try? String(contentsOfFile: binDir + "/jira/com.jira.poll.plist", encoding: .utf8) {
        let want = tmpl.replacingOccurrences(of: "__WS_CONFIG__",
                                             with: home + "/.config/workspace-switcher")
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
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/bin/launchctl")
    p.arguments = args
    p.standardOutput = FileHandle.nullDevice
    p.standardError = FileHandle.nullDevice
    guard (try? p.run()) != nil else { return -1 }
    p.waitUntilExit()
    return p.terminationStatus
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
    var spec = CommandSpec(
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
        vimMode: tri(vars["vim-mode"]) ?? false,
        vimBin: vars["vim-bin"]?.trimmingCharacters(in: .whitespaces) ?? "nvim",
        vimInit: (vars["vim-init"] ?? "").isEmpty ? nil : vars["vim-init"],
        startDrawer: (vars["start-drawer"] ?? "").isEmpty ? "browser"
            : vars["start-drawer"]!.lowercased(),
        fontSize: num(vars["font-size"]),
        escClose: Int(vars["esc-close"] ?? vars["vim-esc-close"] ?? ""),
        imageRows: Int(vars["image-rows"] ?? "") ?? 10,
        maxHeight: num(vars["max-height"]),
        icon: vars["icon"].flatMap(resolveIconName),
        saveDir: (vars["save-dir"] ?? "").isEmpty ? "/tmp/" : vars["save-dir"]!)
    spec.textColor = hexColor(vars["text-color"])
    if let l = vars["label"]?.trimmingCharacters(in: .whitespaces), !l.isEmpty { spec.label = l }
    spec.tabsOpaque = tri(vars["tabs-opaque"])
    spec.dimColor = hexColor(vars["dim-color"])
    spec.highlightColor = hexColor(vars["highlight-color"])
    spec.accentColor = hexColor(vars["accent-color"])
    spec.float = tri(vars["float"])
    spec.sort = vars["sort"]
    if let o = vars["sort-order"]?.lowercased() { spec.sortDescending = o.hasPrefix("desc") }
    spec.searchLimit = Int(vars["search-limit"] ?? "")
    if vars["search-exclude"] != nil { spec.searchExclude = csv(vars["search-exclude"]) }
    if vars["terminal-words"] != nil { spec.terminalWords = csv(vars["terminal-words"]) }
    spec.terminalForeground = hexColor(vars["terminal-foreground"])
    spec.table = tri(vars["table"]) ?? false
    spec.columns = ListColumn.parse(vars["columns"])
    spec.tableSort = vars["table-sort"]
    return spec
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
    // never let a parsed surface color be fully invisible — floor the
    // transparency at 8% so a hand-edited "00…" can't blank a window
    return NSColor(srgbRed: Double((v >> 16) & 0xFF) / 255,
                   green: Double((v >> 8) & 0xFF) / 255,
                   blue: Double(v & 0xFF) / 255, alpha: max(a, 0.08))
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
    guard let content = readConfigText() else { return }
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
    if let v = str("terminal-font-size"), let n = Double(v), n >= 6 { settings.terminalFontSize = CGFloat(n) }
    // font-install-casks = Label|cask|type, Label|cask|type, …
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
    if vars["screenshot-apps"] != nil { settings.screenshotApps = csv(vars["screenshot-apps"]) }
    if let v = str("hide-on-focus-loss") { settings.hideOnFocusLoss = ["true", "yes", "1", "on"].contains(v.lowercased()) }
    if let v = tri(str("float")) { settings.float = v }
    if let v = str("esc-close"), let n = Int(v) { settings.escClose = max(0, n) }
    if vars["copy-toast"] != nil { settings.copyToast = str("copy-toast") ?? "" }
    if let v = str("terminal-app") { settings.terminalApp = v }
}

// MARK: - Theme presets (header icon menu ▸ Theme)

// A coordinated palette for one window. "Whole Window" applies every surface
// (the blended look: notepad, a slightly deeper explorer + terminal, a raised
// header); the per-surface items apply just that surface. Transparency is
// kept per surface — a preset changes hues, never how see-through it is.
// Every color a Theme preset can touch on one window, captured when the
// Theme menu opens so a hover preview can be undone exactly.
struct ThemeSnapshot {
    let roles: [(PopupWindow.ThemeRole, NSColor)]
    let text, dim, highlight, accent: NSColor
    let terminalForeground: NSColor?
    init(_ w: PopupWindow) {
        roles = PopupWindow.ThemeRole.allCases.map { ($0, w.themeColor($0)) }
        text = w.config.colors.text
        dim = w.config.colors.dim
        highlight = w.config.colors.highlight
        accent = w.config.colors.accent
        terminalForeground = w.config.terminalForeground
    }
    func restore(_ w: PopupWindow) {
        for (role, c) in roles { w.setThemeColor(c, for: role) }
        w.setTerminalForeground(terminalForeground)
        w.setTextColors(text: text, dim: dim, highlight: highlight, accent: accent)
    }
}

// Theme menu delegate: reports the highlighted preset (item tag) and the
// menu closing, so presets preview live while hovered.
final class ThemePreviewDelegate: NSObject, NSMenuDelegate {
    private let onHighlight: (Int?) -> Void
    private let onClose: () -> Void
    private var current: Int?
    // set when a real Theme item was picked: closing must not undo it
    var committed = false
    init(onHighlight: @escaping (Int?) -> Void, onClose: @escaping () -> Void) {
        self.onHighlight = onHighlight
        self.onClose = onClose
    }
    func menu(_ menu: NSMenu, willHighlight item: NSMenuItem?) {
        // only preset rows carry a submenu; Custom/Reset rows restore
        let tag = (item?.submenu != nil) ? item?.tag : nil
        guard tag != current else { return }
        current = tag
        onHighlight(tag)
    }
    func menuDidClose(_ menu: NSMenu) {
        current = nil
        if !committed { onClose() }
    }
}

struct ThemePreset {
    let name: String
    let background: NSColor   // notepad / window card
    let browser: NSColor      // file-explorer panel
    let terminal: NSColor     // shell drawer
    let header: NSColor       // drag header
    let text: NSColor
    let dim: NSColor
    let highlight: NSColor    // selection / active pills
    let accent: NSColor       // active tab / chip underline (the theme's signature hue)

    var isLight: Bool { background.relativeLuminance > 0.45 }

    // commands.conf [themes]:  Name = bg, browser, terminal, header, text, dim, highlight[, accent]
    static func parse(name: String, _ value: String) -> ThemePreset? {
        let c = value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard !name.isEmpty, c.count == 7 || c.count == 8 else { return nil }
        let colors = c.compactMap { hexColor($0) }
        guard colors.count == c.count else { return nil }
        return ThemePreset(name: name, background: colors[0], browser: colors[1],
                           terminal: colors[2], header: colors[3], text: colors[4],
                           dim: colors[5], highlight: colors[6],
                           accent: colors.count == 8 ? colors[7] : colors[4])
    }

    // the stock palettes (official hex values where the theme publishes them)
    static let builtIn: [ThemePreset] = [
        ("Tokyo Night", "1A1B26, 16161E, 13141C, 24283B, C0CAF5, 9AA5CE, 283457, 7AA2F7"),
        ("Tokyo Night Storm", "24283B, 1F2335, 1B1E2D, 292E42, C0CAF5, 9AA5CE, 2E3C64, 7AA2F7"),
        ("Catppuccin Mocha", "1E1E2E, 181825, 11111B, 313244, CDD6F4, A6ADC8, 45475A, CBA6F7"),
        ("Catppuccin Macchiato", "24273A, 1E2030, 181926, 363A4F, CAD3F5, A5ADCB, 494D64, C6A0F6"),
        ("Dracula", "282A36, 21222C, 191A21, 343746, F8F8F2, A4AACC, 44475A, BD93F9"),
        ("Nord", "2E3440, 3B4252, 272C36, 434C5E, ECEFF4, A3ACBD, 4C566A, 88C0D0"),
        ("Gruvbox Dark", "282828, 1D2021, 1D2021, 3C3836, EBDBB2, A89984, 504945, FABD2F"),
        ("One Dark", "282C34, 21252B, 1E2127, 2C313A, ABB2BF, 7F848E, 3E4451, 61AFEF"),
        ("Rosé Pine", "191724, 1F1D2E, 16141F, 26233A, E0DEF4, 908CAA, 403D52, EBBCBA"),
        ("Solarized Dark", "002B36, 073642, 00212B, 073642, 93A1A1, 657B83, 0A4A5A, 268BD2"),
        ("Graphite", "1E1E1E, 252525, 181818, 2D2D2D, E5E5E5, 9A9A9A, 3A3A3A, 0A84FF"),
        ("Catppuccin Latte", "EFF1F5, E6E9EF, DCE0E8, CCD0DA, 4C4F69, 6C6F85, BCC0CC, 8839EF"),
        ("Tokyo Night Day", "E1E2E7, D5D6DB, D0D5E3, C4C8DA, 3760BF, 6172B0, B7C1E3, 2E7DE9"),
        ("Solarized Light", "FDF6E3, EEE8D5, EEE8D5, E4DDC8, 586E75, 839496, DDD6C1, 268BD2"),
        ("Paper", "F5F5F5, EDEDED, FFFFFF, E3E3E3, 1D1D1F, 6E6E73, D1D1D6, 007AFF"),
    ].compactMap { parse(name: $0.0, $0.1) }

    // built-ins + commands.conf [themes] entries (same name = override)
    static func all() -> [ThemePreset] {
        var out = builtIn
        guard let content = readConfigText() else { return out }
        var inThemes = false
        for line in content.split(separator: "\n") {
            let s = line.trimmingCharacters(in: .whitespaces)
            if s.isEmpty || s.hasPrefix("#") { continue }
            if s.hasPrefix("[") && s.hasSuffix("]") {
                inThemes = s == "[themes]"
                continue
            }
            guard inThemes, let eq = s.firstIndex(of: "=") else { continue }
            let name = s[..<eq].trimmingCharacters(in: .whitespaces)
            guard let p = parse(name: name, String(s[s.index(after: eq)...])) else { continue }
            if let i = out.firstIndex(where: { $0.name == name }) { out[i] = p } else { out.append(p) }
        }
        return out
    }

    // a small swatch strip for the menu item: background, terminal, text
    func swatch() -> NSImage {
        let size = NSSize(width: 30, height: 14)
        return NSImage(size: size, flipped: false) { r in
            let card = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: 4, yRadius: 4)
            self.background.setFill()
            card.fill()
            NSGraphicsContext.current?.saveGraphicsState()
            card.addClip()
            self.terminal.setFill()
            NSRect(x: r.maxX - 10, y: 0, width: 10, height: r.height).fill()
            NSGraphicsContext.current?.restoreGraphicsState()
            self.text.setFill()
            NSBezierPath(ovalIn: NSRect(x: 5, y: r.midY - 3, width: 6, height: 6)).fill()
            self.accent.setFill()
            NSBezierPath(ovalIn: NSRect(x: 12, y: r.midY - 3, width: 6, height: 6)).fill()
            NSColor.black.withAlphaComponent(0.25).setStroke()
            card.lineWidth = 1
            card.stroke()
            return true
        }
    }
}

extension NSColor {
    // WCAG relative luminance (0 = black, 1 = white)
    var relativeLuminance: CGFloat {
        let c = usingColorSpace(.sRGB) ?? self
        func lin(_ v: CGFloat) -> CGFloat { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(c.redComponent) + 0.7152 * lin(c.greenComponent) + 0.0722 * lin(c.blueComponent)
    }
}

// MARK: - commands.conf store (validated reads + backed-up writes)
//
// Every read of commands.conf goes through readConfigText() and every write
// through writeConfigText(). A file that fails validation (unreadable, empty,
// binary, a malformed [section] header, mostly-garbage lines) is never used:
// the last-known-good copy in commands.conf.bak stands in for it. Each valid
// read refreshes that backup, and an app write that would produce an invalid
// file is refused, so neither a bad hand edit nor an app write can leave the
// app without a working config. Value-level problems (a non-hex color, a
// non-number width) are only warnings: the loader already ignores them and
// falls back to the built-in default.

struct ConfigIssue {
    let line: Int          // 1-based; 0 = whole file
    let message: String
    let fatal: Bool        // true = the file is unusable as a whole
}

var configBackupPath: String { settings.commandsConfPath + ".bak" }
// findings of the latest read (status-bar "Config Issues…" item)
private(set) var configIssues: [ConfigIssue] = []
// true while commands.conf is invalid and the backup is being read instead
private(set) var configUsingBackup = false
// (mtime+size stamp, text handed out) so repeated reads don't re-validate
private var configReadCache: (stamp: String, text: String?)?

private func configLog(_ s: String) {
    let line = "ws: \(s)\n"
    FileHandle.standardError.write(Data(line.utf8))
    if let fh = try? FileHandle(forWritingTo: URL(fileURLWithPath: "/tmp/ws-debug.log")) {
        fh.seekToEndOfFile()
        fh.write(Data(line.utf8))
        try? fh.close()
    }
}

private let configBoolKeys: Set<String> = [
    "enabled", "resize", "drag", "sticky", "voice", "terminal", "vim-mode",
    "checkbox", "hide-on-focus-loss", "float", "table",
]
private let configNumberKeys: [String: ClosedRange<Double>] = [
    "width": 100...8000, "height": 60...8000, "max-height": 60...8000,
    "terminal-height": 40...4000, "font-size": 6...96, "terminal-font-size": 6...96,
    "max-rows": 0...10_000, "page-size": 0...100_000, "content-cap": 0...100_000,
    "body-lines": 0...100, "zoxide-top": 0...100, "search-width": 0...1,
    "tint-alpha": 0...1, "max-row-stretch": 0...1000, "image-rows": 1...200,
    "vim-esc-close": 0...20, "esc-close": 0...20, "search-limit": 1...1_000_000,
    "dashboard-width": 600...8000, "dashboard-height": 400...8000, "dashboard-refresh": 2...3600,
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
    "copy-format": ["tsv"],
]

// why `value` is invalid for `key` in [section], or nil when it's fine.
// Empty values are always allowed (they mean "use the default").
private func configValueProblem(section: String, key: String, value: String) -> String? {
    guard !value.isEmpty else { return nil }
    if section == "theme" {
        return hexColor(value) == nil ? "'\(value)' is not a hex color (RRGGBB / AARRGGBB)" : nil
    }
    if section == "themes" {
        return ThemePreset.parse(name: key, value) == nil
            ? "expected 7 hex colors: background, browser, terminal, header, text, dim, highlight"
            : nil
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
    if configColorKeys.contains(key), hexColor(value) == nil {
        return "'\(value)' is not a hex color (RRGGBB / AARRGGBB)"
    }
    if let allowed = configEnumKeys[key], !allowed.contains(value.lowercased()) {
        return "'\(value)' is not one of \(allowed.sorted().joined(separator: " | "))"
    }
    if key == "columns" {
        // field:Title:width:align:flags — the widths are % of the row
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
    for (i, raw) in text.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
        let n = i + 1
        let s = raw.trimmingCharacters(in: .whitespaces)
        if s.isEmpty || s.hasPrefix("#") { continue }
        if s.hasPrefix("[") {
            // a broken header would silently fold the next keys into the
            // PREVIOUS section (e.g. [notes keys landing in [theme]) — fatal
            let name = s.hasSuffix("]")
                ? String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces) : ""
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
        guard let eq = s.firstIndex(of: "=") else {
            garbage += 1
            warn(n, "ignored line (no '='): \(s.prefix(40))")
            continue
        }
        let key = s[..<eq].trimmingCharacters(in: .whitespaces)
        let val = s[s.index(after: eq)...].trimmingCharacters(in: .whitespaces)
        guard !key.isEmpty else {
            garbage += 1
            warn(n, "missing key before '='")
            continue
        }
        entries += 1
        guard let sec = section else { continue }   // top-level `name = command`
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

// commands.conf as the app should see it: the file itself when it validates,
// otherwise the last-known-good backup (nil when neither is usable).
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
                // nothing on disk to lose: put the backup back in place
                try? backup.write(toFile: path, atomically: true, encoding: .utf8)
                issues = [ConfigIssue(line: 0, message: "\(commandsConfName) was missing — restored from backup", fatal: false)]
                usingBackup = false
            }
            let why = issues.filter(\.fatal)
                .map { ($0.line > 0 ? "line \($0.line): " : "") + $0.message }
                .joined(separator: "; ")
            configLog("commands.conf invalid (\(why)) — using \(configBackupPath)")
        }
        // a rejected file's per-value warnings are mostly fallout of the
        // fatal error (keys folded into the wrong section) — list only it
        issues = issues.filter(\.fatal) + issues.filter { !$0.fatal && attrs == nil }
    } else if let text, text != backup {
        try? text.write(toFile: configBackupPath, atomically: true, encoding: .utf8)
    }
    for i in issues where !i.fatal { configLog("commands.conf:\(i.line): \(i.message)") }
    configIssues = issues
    configUsingBackup = usingBackup
    // re-stat: restoring a missing file changes the stamp
    let a2 = try? FileManager.default.attributesOfItem(atPath: path)
    let m2 = (a2?[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
    configReadCache = ("\(m2)-\((a2?[.size] as? Int) ?? -1)", result)
    return result
}

// Write commands.conf. Refuses content that would not validate; otherwise the
// current file is snapshotted first — a valid one becomes the backup, a broken
// hand edit is parked as commands.conf.broken-<time> so it's never lost.
@discardableResult
func writeConfigText(_ text: String) -> Bool {
    let fatal = validateConfig(text).filter(\.fatal)
    guard fatal.isEmpty else {
        configLog("commands.conf: write refused — \(fatal.map(\.message).joined(separator: "; "))")
        return false
    }
    let path = settings.commandsConfPath
    if let cur = try? String(contentsOfFile: path, encoding: .utf8), cur != text {
        if validateConfig(cur).contains(where: \.fatal) {
            let parked = path + ".broken-\(Int(Date().timeIntervalSince1970))"
            try? cur.write(toFile: parked, atomically: true, encoding: .utf8)
            configLog("commands.conf: invalid file parked at \(parked)")
        } else {
            try? cur.write(toFile: configBackupPath, atomically: true, encoding: .utf8)
        }
    }
    do {
        try text.write(toFile: path, atomically: true, encoding: .utf8)
        return true
    } catch {
        configLog("commands.conf: write failed: \(error)")
        return false
    }
}

// Status-bar "Restore Backup": park the broken file, copy the backup over it.
func restoreConfigFromBackup() -> Bool {
    guard let bak = try? String(contentsOfFile: configBackupPath, encoding: .utf8) else { return false }
    return writeConfigText(bak)
}

// Persist a config value back to commands.conf. Finds the target section,
// updates the key if it exists, or appends it after the section header.
// Preserves all comments, formatting, and other sections untouched.
func saveConfigValue(section: String, key: String, value: String) {
    guard let content = readConfigText() else { return }
    var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    var inTarget = false
    var keyFound = false
    var insertAfter = -1

    for i in 0..<lines.count {
        let s = lines[i].trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("[") && s.hasSuffix("]") {
            let name = String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
            inTarget = name == section
            if inTarget { insertAfter = i }
            continue
        }
        guard inTarget, let eq = s.firstIndex(of: "=") else { continue }
        let k = s[..<eq].trimmingCharacters(in: .whitespaces)
        if k == key {
            lines[i] = "\(key) = \(value)"
            keyFound = true
            break
        }
        insertAfter = i
    }

    if !keyFound, insertAfter >= 0 {
        lines.insert("\(key) = \(value)", at: insertAfter + 1)
    } else if !keyFound {
        // section not found — append it at the end
        lines.append("")
        lines.append("[\(section)]")
        lines.append("\(key) = \(value)")
    }

    let newContent = lines.joined(separator: "\n")
    writeConfigText(newContent)
}

// Set (or, for a nil value, remove) several keys of one [section] in a single
// validated write — a theme preset touches up to 8 keys at once.
func saveConfigValues(section: String, _ kv: [(String, String?)]) {
    guard let content = readConfigText() else { return }
    var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    for (key, value) in kv {
        var header = -1
        var lastInSection = -1
        var found = -1
        var inTarget = false
        for i in 0..<lines.count {
            let s = lines[i].trimmingCharacters(in: .whitespaces)
            if s.hasPrefix("[") && s.hasSuffix("]") {
                inTarget = String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces) == section
                if inTarget { header = i; lastInSection = i }
                continue
            }
            guard inTarget, let eq = s.firstIndex(of: "=") else { continue }
            lastInSection = i
            if s[..<eq].trimmingCharacters(in: .whitespaces) == key { found = i }
        }
        switch (found >= 0, value) {
        case (true, let v?): lines[found] = "\(key) = \(v)"
        case (true, nil): lines.remove(at: found)
        case (false, let v?) where header >= 0: lines.insert("\(key) = \(v)", at: lastInSection + 1)
        case (false, let v?):
            lines += ["", "[\(section)]", "\(key) = \(v)"]
        case (false, nil): break
        }
    }
    writeConfigText(lines.joined(separator: "\n"))
}

// Remove a config key from commands.conf (for reset-to-default).
func removeConfigValue(section: String, key: String) {
    guard let content = readConfigText() else { return }
    var lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    var inTarget = false
    var toRemove: [Int] = []

    for i in 0..<lines.count {
        let s = lines[i].trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("[") && s.hasSuffix("]") {
            let name = String(s.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
            inTarget = name == section
            continue
        }
        guard inTarget, let eq = s.firstIndex(of: "=") else { continue }
        let k = s[..<eq].trimmingCharacters(in: .whitespaces)
        if k == key {
            toRemove.append(i)
            break
        }
    }

    for idx in toRemove.sorted(by: >) {
        lines.remove(at: idx)
    }

    // also remove any trailing empty lines we may have created
    while lines.last?.isEmpty ?? false, lines.count > 1 {
        lines.removeLast()
    }

    let newContent = lines.joined(separator: "\n")
    writeConfigText(newContent)
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

// Resolve a binary by name: checks the PATH, returns the absolute path
// or nil if not found. If the input is already an absolute path, returns
// it directly if it exists.
private func resolveBinary(_ name: String) -> String? {
    if name.hasPrefix("/") {
        return FileManager.default.isExecutableFile(atPath: name) ? name : nil
    }
    let paths = (ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin")
        .split(separator: ":").map(String.init)
    for dir in paths {
        let fullPath = dir + "/" + name
        if FileManager.default.isExecutableFile(atPath: fullPath) {
            return fullPath
        }
    }
    return nil
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
// the Jira site for "open in browser": config.json (python poller) first,
// then the legacy env-style config. Computed on every use — the setup sheet
// can change it while the app runs.
var jiraSite: String {
    if let data = try? Data(contentsOf: URL(fileURLWithPath: JiraPoll.configPath)),
       let d = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
       let site = d["site"] as? String, !site.isEmpty {
        return site.hasSuffix("/") ? String(site.dropLast()) : site
    }
    let conf = NSString(string: "~/.config/jira/config").expandingTildeInPath
    guard let s = try? String(contentsOfFile: conf, encoding: .utf8) else { return "" }
    for line in s.split(separator: "\n") where line.hasPrefix("JIRA_SITE=") {
        return String(line.dropFirst("JIRA_SITE=".count))
            .trimmingCharacters(in: CharacterSet(charactersIn: "'\""))
    }
    return ""
}

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
    init(_ c: CommandSpec) { title = "> \(c.label ?? c.name)"; command = c }
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
    // list/table cells show ISO timestamps trimmed to local minutes; the raw
    // value (ms + offset) stays in `fields` for sort, filters and the
    // double-click detail window
    func cellText(_ field: String) -> String? { fields[field].map(compactTimestamp) }
}

// "2026-09-13T11:54:04.850-0400" -> "2026-09-13 11:54" (local time). Anything
// that is not an ISO-8601 date-time passes through untouched; the cheap shape
// check keeps per-cell drawing fast.
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
func compactTimestamp(_ s: String) -> String {
    let u = Array(s.utf8)
    guard u.count >= 19, u.count <= 35, u[4] == 45, u[7] == 45, u[10] == 84, u[13] == 58
    else { return s }
    for p in isoParsers {
        if let d = p.date(from: s) { return compactStampFormatter.string(from: d) }
    }
    return s
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
    // the open jira window's "show this tab (freshly loaded)" hook — the live
    // search panel calls it after a run (see openListWindow)
    var jiraShowTab: ((String) -> Void)?
    var pendingJiraTab: String?
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
    // the picked hue (always opaque — the color wheel never changes
    // transparency) and the surface's opacity (0-1, from the dedicated slider)
    private var pickerHue: NSColor = .clear
    private var pickerTransparency: CGFloat = 0.0    // 0 = opaque, 1 = transparent
    private var pickerHexLabel: NSTextField?
    private var pickerTransparencyLabel: NSTextField?
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
        startScreenshotYield()
    }

    // Our windows float at the pop-up-menu level, above everything. Tools like
    // Flameshot grab the screen first, then show that frozen image in an
    // ordinary (level 0) overlay — so the live popup kept covering it and the
    // selection rectangle looked like it was drawn BEHIND our window. While
    // such a tool is the active app our visible popups go fully transparent
    // (the frozen image already contains them, so the shot is unchanged);
    // they come back as soon as any other app activates.
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
            guard let self else { return }
            // consume EVERY bridge write, even while we're already active:
            // a write left unconsumed (notes focused while active) used to
            // fire later — switching workspaces 4 -> 1 deactivated us, the
            // poller saw the stale "notes focused" write, re-activated the
            // notes window and aerospace jumped straight back to 4
            var st = stat()
            guard stat(path, &st) == 0 else { return }
            let mt = (Int(st.st_mtimespec.tv_sec), Int(st.st_mtimespec.tv_nsec))
            if let prev = self.bridgeMtime, prev.0 == mt.0, prev.1 == mt.1 { return }
            self.bridgeMtime = mt
            guard !NSApp.isActive,
                  self.subWindows.contains(where: { $0.isShown }) else { return }
            if let last = self.lastOtherAppClick,
               Date().timeIntervalSince(last) < 1.0 { return }
            // only a FRESH write means aerospace just focused us — never act
            // on one that sat around (e.g. the first tick after launch)
            let age = Date().timeIntervalSince1970
                - (Double(mt.0) + Double(mt.1) / 1_000_000_000)
            guard age < 1.0 else { return }
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
                var buf = [UInt8](repeating: 0, count: 2048)
                let n = read(cfd, &buf, buf.count)
                close(cfd)
                if n > 0 {
                    let msg = String(bytes: buf[..<n], encoding: .utf8) ?? ""
                    let name = msg.trimmingCharacters(in: .whitespacesAndNewlines)
                    DispatchQueue.main.async { [weak self] in
                        if name == "reset-size" {
                            self?.noteWindow?.resetToDefaultSize()
                        } else if name == "toggle-terminal" || name == "toggle-browser" {
                            // drawer toggles on the notes window (scripts/tests)
                            guard let w = self?.noteWindow else { return }
                            if name == "toggle-terminal" { w.toggleTerminalDrawer() } else { w.toggleFileBrowser() }
                        } else if name.hasPrefix("open:") {
                            // "open:<absolute path>" opens that file as a
                            // notes tab (same as Finder's "Open in Notes")
                            let path = String(name.dropFirst(5))
                            self?.openNoteFile((path as NSString).expandingTildeInPath)
                        } else if name == "notes" {
                            self?.showNotes()
                        } else if name == "jira-dashboard" {
                            self?.showJiraDashboard()
                        } else if name.hasPrefix("jira-poll-") || name == "jira-setup" {
                            // THE jira switch (menu-bar "Enable Jira"/"Disable Jira")
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
            // [jira-config] only exists while Jira is enabled
            let jira = jiraEnabledInConfig()
            let cmds = PopupFuzzy.filter(commands.filter { $0.name != "jira-config" || jira }, query: sub) { c in
                c.label.map { "\($0) \(c.name)" } ?? c.name
            }
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
                // /jira-config opens the Jira Config window (in-process too)
                if cmd.name == "jira-config" {
                    showJiraDashboard()
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

    func log(_ s: String) {
        FileHandle.standardError.write(Data("ws: \(s)\n".utf8))
        let p = "/tmp/ws-debug.log"
        if let fh = try? FileHandle(forWritingTo: URL(fileURLWithPath: p)) {
            fh.seekToEndOfFile()
            fh.write(Data("ws: \(s)\n".utf8))
            try? fh.close()
        } else {
            try? "ws: \(s)\n".write(to: URL(fileURLWithPath: p), atomically: true, encoding: .utf8)
        }
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

    // Toggle vim mode for the notes window: updates the command spec, persists
    // to commands.conf, and relaunches the notes window if one is open so the
    // change takes effect immediately.
    func toggleVimModeForNotes() {
        guard let idx = commands.firstIndex(where: { $0.name == "notes" }) else { return }
        let cmd = commands[idx]
        let newValue = !cmd.vimMode
        log("vim mode: \(newValue ? "enabled" : "disabled") for notes")

        // Update the command spec in the in-memory array
        commands[idx].vimMode = newValue

        // Persist to commands.conf
        saveConfigValue(section: "notes", key: "vim-mode", value: newValue ? "true" : "false")

        // rebuild the open notes window in the new mode (the note is
        // flushed first, so nothing typed is lost)
        rebuildNoteWindow()
    }

    // Launch args for the notes vim pane. The bundled vim/notes-init.vim
    // (chrome-less, autosaving, transparent) is used unless commands.conf
    // `vim-init` names another file; with the bundled init, personal plugins
    // are skipped so a broken plugin can never block the pane with a
    // "Press ENTER" prompt. Theme colors are handed in as g:ws_* variables.
    func vimArgs(for cmd: CommandSpec, socket: String, file: String?) -> [String] {
        var a: [String] = []
        let isNvim = (cmd.vimBin as NSString).lastPathComponent.hasPrefix("nvim")
        if isNvim { a += ["--listen", socket] }
        let custom = cmd.vimInit.map { ($0 as NSString).expandingTildeInPath }
        if let custom, FileManager.default.fileExists(atPath: custom) {
            a += ["-u", custom]
        } else {
            let bundled = binDir + "/vim/notes-init.vim"
            if FileManager.default.fileExists(atPath: bundled) {
                a += ["--noplugin", "-u", bundled]
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
        a += ["--cmd", "let g:ws_img_file='\(imgFile)'",
              "--cmd", "let g:ws_img_rows=\(max(1, cmd.imageRows))"]
        a += ["--cmd", "let g:ws_fg='\(rgb(cmd.textColor ?? TEXT))'",
              "--cmd", "let g:ws_dim='\(rgb(cmd.dimColor ?? DIM))'",
              "--cmd", "let g:ws_sel='\(rgb(cmd.highlightColor ?? GROUP_BG))'"]
        // cursor-line band (iTerm2-style cursor guide): the selection color
        // pulled halfway toward the card so it reads fainter than Visual
        let sel = (cmd.highlightColor ?? GROUP_BG).usingColorSpace(.sRGB) ?? GROUP_BG
        let card = (cmd.backgroundColor ?? BAR).withAlphaComponent(1).usingColorSpace(.sRGB) ?? BAR
        a += ["--cmd", "let g:ws_line='\(rgb(sel.blended(withFraction: 0.5, of: card) ?? sel))'"]
        if let file { a.append(file) }
        return a
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
        cfg.floating = cmd.float ?? settings.float
        cfg.escCloseCount = max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
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
        cfg.floating = cmd.float ?? settings.float
        cfg.escCloseCount = max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
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
        cfg.floating = cmd.float ?? settings.float
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
        cfg.floating = cmd.float ?? settings.float
        cfg.escCloseCount = max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
        cfg.tabs = true
        cfg.tabsAddButton = true
        cfg.opaqueTabs = cmd.tabsOpaque ?? true
        cfg.width = cmd.width > 0 ? cmd.width : defaultNoteSize.width
        // `start-drawer` (browser | terminal | none) picks the pane open on
        // launch; the initial height folds in whichever drawer opens
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
        // slim header (same height as the jira detail window): no title pill,
        // bluey-silver strip, app glyph far left with the last-write line
        cfg.headerHeight = 30
        cfg.titlePill = false
        // the header buttons fill the whole top strip (right rounded edge back
        // to the app glyph / last-write line) instead of a compact right cluster
        cfg.stretchHeaderButtons = true
        cfg.headerColor = cmd.headerColor ?? headerBlueSilver
        cfg.colors = PopupColors(background: BAR, border: BORDER,
                                 text: cmd.textColor ?? TEXT, dim: cmd.dimColor ?? DIM,
                                 highlight: cmd.highlightColor ?? GROUP_BG,
                                 accent: cmd.accentColor ?? ACCENT)
        cfg.terminalForeground = cmd.terminalForeground
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

        // vim mode: an embedded nvim pane replaces the text view (tabs,
        // drawers and chrome stay). One long-lived editor; tab switches go
        // over its --listen socket, so nothing quits or relaunches.
        let vimSocket = NSHomeDirectory()
            + "/.cache/workspace-switcher/nvim-\(cmd.name)-\(getpid()).sock"
        // inline-image placements the editor writes (vim/notes-init.vim)
        let vimImageFile = (vimSocket as NSString).deletingPathExtension + ".images.json"
        if cmd.vimMode {
            let exe = resolveBinary(cmd.vimBin) ?? cmd.vimBin
            cfg.vimEditorExecutable = exe
            cfg.vimEditorSocket = vimSocket
            cfg.vimImageFile = vimImageFile
            cfg.vimEditorArgs = vimArgs(for: cmd, socket: vimSocket,
                                        file: noteIsPreview(currentPath) ? nil : currentPath)
            log("note '\(cmd.name)': vim pane \(exe) socket \(vimSocket)")
        }

        let w = PopupWindow(config: cfg)
        // All window actions now live in the top-left icon dropdown menu —
        // no scattered header buttons. The menu shows toggle state via
        // checkmarks (terminal, browser, mic) and groups actions logically.

        // image saving for pasted/dropped photos
        func noteDir(_ p: String) -> String { (p as NSString).deletingLastPathComponent }
        w.imageBaseDir = noteDir(currentPath)
        if noteIsPreview(currentPath) {
            w.setEditorFilePreview(currentPath)
        } else {
            w.editorReadOnly = false
            w.setEditorMarkdown(content, baseDir: noteDir(currentPath))
        }
        // vim pane: text notes edit in vim; PDF/image tabs keep the native
        // preview. A relaunched editor (after `:q`) reopens the current note.
        if cmd.vimMode {
            w.setVimPaneActive(!noteIsPreview(currentPath))
            w.vimLaunchArgs = { [weak self] in
                self?.vimArgs(for: cmd, socket: vimSocket,
                              file: noteIsPreview(currentPath) ? nil : currentPath) ?? []
            }
            w.onVimExit = { [weak self] in
                self?.log("note '\(cmd.name)': vim exited — relaunching on \(currentPath)")
            }
            // right-click in the vim pane: the obvious actions (rule 2)
            let vm = NSMenu(title: "Vim")
            vm.autoenablesItems = false
            vm.addItem(menuItem("Copy") { [weak w] in w?.vimCopy() })
            vm.addItem(menuItem("Paste") { [weak w] in w?.vimPaste() })
            vm.addItem(.separator())
            vm.addItem(menuItem("Copy File Path") { [weak self] in
                self?.copy(currentPath, "note path: \(currentPath)")
            })
            vm.addItem(menuItem("Open in Default App") {
                NSWorkspace.shared.open(URL(fileURLWithPath: currentPath))
            })
            vm.addItem(menuItem("Reveal in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: currentPath)])
            })
            vm.addItem(menuItem("Open file at path…") { [weak w] in w?.onOpenPathPrompt?() })
            w.vimMenu = vm
        }
        w.onFontSizeStep = { [weak self] delta in self?.stepFontSizes(delta) }
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
            // Vim mode: the editor owns the file — flush it, never write the
            // (hidden, stale) text view over it.
            if cmd.vimMode {
                w.vimFlush()
            } else if FileManager.default.fileExists(atPath: outgoing), !noteIsPreview(outgoing) {
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
            if cmd.vimMode {
                // vim pane follows the tab; previews fall back to the native
                // read-only view
                let preview = noteIsPreview(currentPath)
                if preview { w.setEditorFilePreview(currentPath) } else { w.vimOpen(currentPath) }
                w.setVimPaneActive(!preview)
                lastSynced = ""
            } else if noteIsPreview(currentPath) {
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
            if cmd.vimMode {
                w.vimFlush()
            } else if wasCurrent, FileManager.default.fileExists(atPath: closing),
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
        // Top-left icon opens a dropdown menu with all window actions —
        // replaces the scattered header buttons (terminal, browser, mic, color).
        w.onChromeIconClick = { [weak self, weak w] in
            guard let self, let w else { return }
            let menu = NSMenu()
            menu.autoenablesItems = false

            // — toggles (checkmark shows state) —
            func toggleItem(_ title: String, _ state: Bool, _ action: @escaping () -> Void) {
                let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
                item.state = state ? .on : .off
                item.target = nil
                item.action = nil
                // Use a closure-based approach: NSMenuItem can't hold closures
                // directly, so we use a target/action pair
                let t = MenuActionTarget(action: action)
                item.target = t
                item.action = #selector(MenuActionTarget.run)
                // Retain the target so it survives the menu dismiss
                menuActionTargets.append(t)
                menu.addItem(item)
            }

            if cmd.terminal {
                toggleItem("Toggle Terminal", w.terminalShown) {
                    w.toggleTerminalDrawer()
                }
            }
            toggleItem("Toggle File Browser", w.fileBrowserShown) {
                w.toggleFileBrowser()
            }
            if cmd.voice {
                let micShown = w.meterEnabled
                toggleItem(micShown ? "Mute Microphone" : "Enable Microphone", micShown) {
                    let shown = !w.meterEnabled
                    w.meterEnabled = shown
                }
            }
            // Jira: "Enable Jira" flips [jira] enabled through the checked
            // path (config check, login test, setup window) and opens the
            // window; once enabled it becomes a window toggle like the drawers
            if jiraEnabledInConfig() {
                let shown = self.subWindows.first(where: { $0.config.name == "jira" })?.isShown ?? false
                toggleItem("Toggle Jira", shown) {
                    self.toggleCommand("jira")
                }
                menu.addItem(self.menuItem("Open Jira Config Window") { [weak self] in
                    self?.showJiraDashboard()
                })
            } else {
                toggleItem("Enable Jira", false) {
                    self.enableJiraChecked()
                }
            }
            // Vim mode toggle — reads the LIVE spec (cmd is this window's
            // launch snapshot)
            if cmd.kind == .note {
                let vimOn = self.noteCommandIndex.map { self.commands[$0].vimMode } ?? cmd.vimMode
                toggleItem("Vim Mode", vimOn) {
                    self.toggleVimModeForNotes()
                }
            }
            menu.addItem(.separator())
            // Font ▸ (editor / terminal family by type, size, install)
            let fontMenu = NSMenu(title: "Font")
            self.buildFontMenu(into: fontMenu)
            let fontItem = NSMenuItem(title: "Font", action: nil, keyEquivalent: "")
            fontItem.submenu = fontMenu
            menu.addItem(fontItem)
            // Notes Settings ▸ (vim binary, start drawer, voice, sticky, …)
            let notesMenu = NSMenu(title: "Notes Settings")
            self.buildNotesSettingsMenu(into: notesMenu)
            let notesItem = NSMenuItem(title: "Notes Settings", action: nil, keyEquivalent: "")
            notesItem.submenu = notesMenu
            menu.addItem(notesItem)
            menu.addItem(.separator())

            menu.addItem(self.focusLossMenuItem(for: w, section: cmd.name))
            menu.addItem(self.floatMenuItem(for: w, section: cmd.name))
            menu.addItem(.separator())
            // — theme presets + transparency —
            self.addThemeMenus(to: menu, window: w, section: cmd.name)
            menu.addItem(.separator())

            // — reset actions —
            let resetSizeItem = NSMenuItem(title: "Reset Default Size", action: nil, keyEquivalent: "")
            let rsTarget = MenuActionTarget { w.resetToDefaultSize() }
            resetSizeItem.target = rsTarget
            resetSizeItem.action = #selector(MenuActionTarget.run)
            menuActionTargets.append(rsTarget)
            menu.addItem(resetSizeItem)

            let resetColorItem = NSMenuItem(title: "Reset Default Colors", action: nil, keyEquivalent: "")
            let rcTarget = MenuActionTarget { self.resetWindowTheme(w, section: cmd.name) }
            resetColorItem.target = rcTarget
            resetColorItem.action = #selector(MenuActionTarget.run)
            menuActionTargets.append(rcTarget)
            menu.addItem(resetColorItem)
            menu.addItem(.separator())

            // — open config —
            let configItem = NSMenuItem(title: "Open Config", action: nil, keyEquivalent: "")
            let cfgTarget = MenuActionTarget {
                let canonical = NSHomeDirectory() + "/.config/workspace-switcher/commands.conf"
                let p = FileManager.default.fileExists(atPath: canonical)
                    ? canonical
                    : settings.commandsConfPath
                if FileManager.default.fileExists(atPath: p) {
                    w.onOpenExternalPath?(p)
                }
            }
            configItem.target = cfgTarget
            configItem.action = #selector(MenuActionTarget.run)
            menuActionTargets.append(cfgTarget)
            menu.addItem(configItem)

            w.showHeaderMenu(menu)
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
                case .idle where cmd.vimMode:
                    // vim mode: batches append straight into the editor's
                    // buffer (vimAppend) — no text-view tail math needed
                    draft = ""
                    voice.start()
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
                if cmd.vimMode {
                    // the live draft shows in the footer; only finalized
                    // batches touch the note
                    draft = text
                    w.tabFooterText = "🎙 " + String(text.suffix(80))
                    return
                }
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
                if cmd.vimMode {
                    if !trimmed.isEmpty {
                        // blank separator line + the batch, appended IN the
                        // editor (then :wall) so it never races typing
                        if !w.vimAppend("\n" + trimmed, to: currentPath) {
                            // no RPC (plain vim): append on disk, reload
                            let old = (try? String(contentsOfFile: currentPath,
                                                   encoding: .utf8)) ?? ""
                            var new = old
                            if !new.isEmpty && !new.hasSuffix("\n") { new += "\n" }
                            new += "\n" + trimmed + "\n"
                            try? new.write(toFile: currentPath, atomically: true, encoding: .utf8)
                            w.vimCommand("silent! checktime")
                        }
                        dbg("vim batch +\(trimmed.count)")
                    } else if voice.state == .transcribing {
                        self.log("voice '\(cmd.name)': no speech detected")
                    }
                    draft = ""
                    w.tabFooterText = lastWriteLabel(currentPath)
                    if voice.state == .transcribing { voice.resetSession() }
                    return
                }
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
            // vim mode: the editor owns the file; `text` is the hidden text
            // view's stale copy — flush the editor instead of writing it
            if cmd.vimMode {
                w.vimFlush()
                lastMtime = mtime(of: currentPath)
                w.tabFooterText = lastWriteLabel(currentPath)
                return
            }
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
                    // vim mode: the live text is the editor's buffer
                    let vimText = cmd.vimMode && !noteIsPreview(p)
                        ? w.vimEval("join(getline(1, '$'), \"\\n\")") : nil
                    let text = noteIsPreview(p) ? "" : (vimText ?? w.currentEditorText)
                    self.log("note '\(cmd.name)': \(p) deleted on disk — text parked in \(fallback)")
                    self.saveNote(text, to: fallback, cmd: cmd)
                    currentPath = fallback
                    lastSynced = text
                    lastMtime = mtime(of: fallback)
                    w.tabFooterText = lastWriteLabel(fallback)
                    w.setEditorMarkdown(text, baseDir: noteDir(fallback))
                    w.imageBaseDir = noteDir(fallback)
                    if cmd.vimMode {
                        // drop the dead buffer (never rewrite the deleted
                        // file) and edit the parked copy
                        w.vimCommand("silent! bwipeout! " + p.replacingOccurrences(of: " ", with: "\\ "))
                        w.vimOpen(fallback)
                        w.setVimPaneActive(true)
                    }
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
            if cmd.vimMode, let mt = mtime(of: currentPath), !noteIsPreview(currentPath) {
                // vim reloads external writes itself (autoread + checktime);
                // an unmodified buffer reloads silently
                if let last = lastMtime, mt != last {
                    w.vimCommand("silent! checktime")
                    w.tabFooterText = lastWriteLabel(currentPath)
                }
                lastMtime = mt
            } else if let mt = mtime(of: currentPath), !noteIsPreview(currentPath) {
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
        // embedded file browser drawer (header "▤" toggles it): starts in the
        // note directory, favorites shared with the floating "files" window
        let favs = fileBrowserFavoritesConfig()
        var browserCfg = cfg
        applyBrowserSettings(&browserCfg)
        let fb = PopupFileBrowser(config: browserCfg, startDir: noteDir(currentPath),
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
        fb.onSortChange = { [weak self] key, desc in self?.saveBrowserSort(key, desc) }
        w.onOpenExternalTerminal = { [weak self] dir in self?.openInTerminalApp(dir) }
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
        if (path as NSString).standardizingPath
            == (settings.commandsConfPath as NSString).standardizingPath {
            reportConfigEdit(text)
        }
    }

    // editing commands.conf as a note: surface validation problems in the
    // window's status strip as you save (the running app keeps its config;
    // an invalid file is replaced by the backup at the next launch)
    private func reportConfigEdit(_ text: String) {
        let issues = validateConfig(text)
        if let f = issues.first(where: \.fatal) {
            let at = f.line > 0 ? "line \(f.line): " : ""
            noteWindow?.setStatus("commands.conf \(at)\(f.message) — the last good backup will be used until this is fixed",
                                  isError: true)
        } else if let w = issues.first {
            let more = issues.count > 1 ? " (+\(issues.count - 1) more)" : ""
            noteWindow?.setStatus("commands.conf line \(w.line): \(w.message)\(more)", isError: false)
        } else {
            noteWindow?.setStatus(nil, isError: false)
        }
    }

    // keep commands.conf in sync: append a newly created note to the [notes]
    // section's paths= line, using the tilde form for paths under $HOME
    private func addNotePathToConfig(_ path: String, section: String) {
        let confPath = settings.commandsConfPath
        guard let content = readConfigText() else {
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
            writeConfigText(lines.joined(separator: "\n"))
            log("commands.conf: added note \(display)")
            return
        }
        log("commands.conf: no [\(section)] section to update")
    }

    // drop a deleted note from commands.conf so it never gets listed again
    // (paths= entries that no longer exist on disk are removed)
    private func removeNotePathFromConfig(_ path: String, section: String) {
        let confPath = settings.commandsConfPath
        guard let content = readConfigText() else {
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
            writeConfigText(lines.joined(separator: "\n"))
            log("commands.conf: removed \(display) from [\(section)]")
            return
        }
        log("commands.conf: no [\(section)] section to update")
    }

    // MARK: Theme presets + transparency (header icon menu)

    enum ThemeScope {
        case window, notepad, terminal, browser
    }
    // the open Theme menu's hover-preview delegate (NSMenu holds it weakly)
    private var themePreviewDelegate: ThemePreviewDelegate?

    // surfaces this window can style: notes = all four, files = the explorer
    private func themeScopes(for w: PopupWindow) -> [(ThemeScope, String)] {
        var out: [(ThemeScope, String)] = [(.window, "Whole Window")]
        if w.config.editMode { out.append((.notepad, "Notepad Only")) }
        if w.config.editMode && w.hasTerminalDrawer { out.append((.terminal, "Terminal Only")) }
        if w.config.editMode && w.hasFileBrowser { out.append((.browser, "File Explorer Only")) }
        return out
    }

    // Theme ▸ (presets, each with a Whole Window / per-surface submenu),
    // Transparency ▸, Custom Color…, Reset. Added to both header icon menus.
    func addThemeMenus(to menu: NSMenu, window w: PopupWindow, section: String) {
        let scopes = themeScopes(for: w)
        let themeMenu = NSMenu(title: "Theme")
        let currentBg = hexString(w.themeColor(.notepad).withAlphaComponent(1))
        let currentBrowser = hexString(w.themeColor(.browser).withAlphaComponent(1))
        let windowTextIsLight = w.config.colors.text.relativeLuminance > 0.45
        let presets = ThemePreset.all()
        // hovering a preset previews it live on this window; closing the
        // menu without picking one puts the current look back
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
        for (i, p) in presets.enumerated() {
            if i > 0, p.isLight, !presets[i - 1].isLight {
                themeMenu.addItem(.separator())
            }
            let item = NSMenuItem(title: p.name, action: nil, keyEquivalent: "")
            item.tag = i
            item.image = p.swatch()
            let matches = w.config.editMode ? currentBg == hexString(p.background)
                                            : currentBrowser == hexString(p.background)
            item.state = matches ? .on : .off
            let sub = NSMenu(title: p.name)
            sub.autoenablesItems = false   // keep unreadable combos greyed
            for (scope, label) in scopes {
                // explorer-only keeps the window's text color, so it only
                // offers presets that stay readable under it
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
            item.submenu = sub
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

        // Transparency ▸ <surface> ▸ level
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

    private func themeRoles(for scope: ThemeScope, window w: PopupWindow) -> [PopupWindow.ThemeRole] {
        switch scope {
        case .window:
            var r: [PopupWindow.ThemeRole] = [.notepad, .header]
            if w.hasFileBrowser { r.append(.browser) }
            if w.hasTerminalDrawer { r.append(.terminal) }
            // the files window IS its explorer: lead with it (menu checkmarks)
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

    // keep the in-memory spec in step with commands.conf so a window rebuild
    // (vim toggle, Start With…) keeps the look
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

    // live-apply + persist color keys for a window (nil = remove the key)
    private func commitColors(_ w: PopupWindow, section: String, _ kv: [(String, NSColor?)]) {
        updateSpecColors(section: section, kv)
        saveConfigValues(section: section, kv.map { ($0.0, $0.1.map(hexString)) })
        log("commands.conf [\(section)]: " + kv.map { "\($0.0)=\($0.1.map(hexString) ?? "-")" }.joined(separator: " "))
    }

    func applyThemePreset(_ p: ThemePreset, scope: ThemeScope,
                          to w: PopupWindow, section: String, persist: Bool = true) {
        var kv: [(String, NSColor?)] = []
        // swap the hue, keep the surface's current transparency — but never
        // below presetMinOpacity: a near-invisible surface (e.g. a terminal at
        // 8%) showed only the desktop blur, so every preset looked the same
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
            w.setTextColors(text: p.text, dim: p.dim, highlight: p.highlight, accent: p.accent)
            kv += [("text-color", p.text), ("dim-color", p.dim), ("highlight-color", p.highlight),
                   ("accent-color", p.accent)]
        }
        guard persist else { return }
        commitColors(w, section: section, kv)
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

    // drop every per-window color key and live-restore the [theme] defaults
    // (text colors included)
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
        w.setTextColors(text: TEXT, dim: DIM, highlight: GROUP_BG, accent: ACCENT)
        NSColorPanel.shared.orderOut(nil)
        log("theme reset for [\(section)] — back to system defaults")
    }

    // "Float Above Other Windows" (header icon menu): per-window `float`.
    // On = stays above every app (default); off = a normal window.
    func floatMenuItem(for w: PopupWindow, section: String) -> NSMenuItem {
        let on = w.config.floating
        return menuItem("Float Above Other Windows", state: on) { [weak self, weak w] in
            guard let self, let w else { return }
            w.setFloating(!on)
            if let i = self.commands.firstIndex(where: { $0.name == section }) {
                self.commands[i].float = !on
            }
            saveConfigValue(section: section, key: "float", value: on ? "false" : "true")
            self.log("[\(section)] float = \(!on)")
        }
    }

    // global default for every popup (menu bar ▸ Settings ▸ Float Windows):
    // applies live to windows without their own `float` key
    func setGlobalFloat(_ on: Bool) {
        settings.float = on
        saveConfigValue(section: "app", key: "float", value: on ? "true" : "false")
        for w in subWindows {
            let own = commands.first(where: { $0.windowName == w.config.name })?.float
            w.setFloating(own ?? on)
        }
    }

    // "Hide When Focus Is Lost" (header icon menu): the per-window inverse of
    // `sticky`. Esc always dismisses; this adds hiding when another app takes
    // focus. Turning it on also re-enables the global [app] switch.
    func focusLossMenuItem(for w: PopupWindow, section: String) -> NSMenuItem {
        let on = !w.config.sticky && settings.hideOnFocusLoss
        return menuItem("Hide When Focus Is Lost", state: on) { [weak self, weak w] in
            guard let self, let w else { return }
            let hide = !on
            w.config.sticky = !hide
            if let i = self.commands.firstIndex(where: { $0.name == section }) {
                self.commands[i].sticky = !hide
            }
            saveConfigValue(section: section, key: "sticky", value: hide ? "false" : "true")
            if hide && !settings.hideOnFocusLoss {
                settings.hideOnFocusLoss = true
                saveConfigValue(section: "app", key: "hide-on-focus-loss", value: "true")
            }
            self.log("[\(section)] hide on focus loss = \(hide)")
        }
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
                // dim the label so it reads as disabled
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
        resetWindowTheme(w, section: pickerSection)
    }

    private func startColorPicker(for w: PopupWindow, role: PopupWindow.ThemeRole) {
        pickerWindow = w
        pickerRole = role
        pickerOriginal = w.themeColor(role)
        pickerCommitted = false
        pickerSawVisible = false
        let seed = (pickerOriginal ?? .clear).usingColorSpace(.sRGB) ?? .clear
        // seed the picker from the ACTUAL current surface: hue + transparency
        pickerHue = seed.withAlphaComponent(1)
        pickerTransparency = 1.0 - seed.alphaComponent
        let panel = NSColorPanel.shared
        panel.mode = .wheel
        panel.color = pickerHue
        panel.showsAlpha = false
        panel.isContinuous = true
        panel.setTarget(self)
        panel.setAction(#selector(panelColorChanged(_:)))
        // accessory: hex readout, transparency slider, Apply / Cancel
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

    // preview hue @ transparency on the window (nothing is written until
    // Apply). The 0.08 floor keeps a surface from ever becoming fully
    // invisible.
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
        guard pickerWindow != nil, pickerRole != nil else { return }
        let alpha = max(1.0 - pickerTransparency, 0.08)
        let color = pickerHue.withAlphaComponent(alpha)
        let hex = hexString(color)
        guard !hex.isEmpty, !pickerSection.isEmpty else { return }
        // per-window override keys — the pick only affects THIS window
        let key: String
        switch pickerRole! {
        case .browser: key = "browser-background"
        case .terminal: key = "terminal-background"
        case .notepad: key = "background-color"
        case .header: key = "header-color"
        }
        updateColorKeyInConfig(hex, key: key, section: pickerSection)
        updateSpecColors(section: pickerSection, [(key, color)])
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
        guard let content = readConfigText() else {
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
                writeConfigText(lines.joined(separator: "\n"))
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
        writeConfigText(lines.joined(separator: "\n"))
    }

    // drop every color-override key from a [section] so the [theme] defaults
    // apply again ("Reset to system defaults" in the picker menu)
    private func removeColorKeysFromConfig(section: String) {
        let confPath = settings.commandsConfPath
        guard let content = readConfigText() else {
            log("commands.conf: cannot read \(confPath)")
            return
        }
        let keys: Set<String> = ["header-color", "background-color",
                                 "browser-background", "terminal-background",
                                 "tint-alpha", "text-color", "dim-color",
                                 "highlight-color", "accent-color", "terminal-foreground"]
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
        writeConfigText(out.joined(separator: "\n"))
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
        // jira: each tab (json file) belongs to a poll job or the live search
        // in config.json with its OWN columns; [jira] columns is the fallback
        func tabColumns(_ path: String?) -> [ListColumn] {
            guard cmd.table else { return [] }
            guard cmd.name == "jira" else { return cmd.columns }
            if let path, let own = JiraPoll.owner(ofTab: path), !own.columns.isEmpty {
                return JiraPoll.labeled(own.columns)
            }
            return JiraPoll.labeled(cmd.columns)
        }
        var tabs: [(path: String, items: [FieldRow])] =
            expandPaths(cmd.sources, extensions: ["json", "tsv"]).map { path in
                return (path, loadListItems(path, cmd: cmd, columns: tabColumns(path)))
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
        cfg.floating = cmd.float ?? settings.float
        cfg.escCloseCount = max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
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
        // jira: rows are acted on through Cmd+K (copy / open in browser),
        // so the header's "copy selected" button goes
        cfg.copyRowsButton = cmd.name != "jira"
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
        // table mode (`table = true` + `columns`): spreadsheet rows under a
        // sticky, sortable, resizable header; absent columns = preview rows
        var columns = tabColumns(tabs.first?.path)
        if !columns.isEmpty {
            cfg.tableColumns = columns.map { $0.popup }
            cfg.rowHeight = 26
        }
        let w = PopupWindow(config: cfg)
        // empty `title` in commands.conf = no header label (icon still shows)
        w.chromeHeaderTitle = cmd.chromeTitle.isEmpty ? nil : cmd.chromeTitle
        w.headerIcon = jiraAppIcon
        // header-click sort: (field, ascending); restored from table-sort
        var sortKey: (field: String, ascending: Bool)?
        if let ts = cmd.tableSort, !columns.isEmpty {
            let parts = ts.split(separator: ":").map { $0.trimmingCharacters(in: .whitespaces) }
            if let f = parts.first, columns.contains(where: { $0.field == f }) {
                sortKey = (f, !(parts.count > 1 && parts[1].lowercased().hasPrefix("desc")))
            }
        }
        func syncSortArrow() {
            w.tableSort = sortKey.flatMap { k in
                columns.firstIndex(where: { $0.field == k.field }).map { ($0, k.ascending) }
            }
        }
        syncSortArrow()
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
        // Cmd+K: act on the ticked rows (else the highlighted one) — copy,
        // and for jira open every issue in the browser
        w.onCommandK = { [weak self, weak w] in
            guard let self, let w else { return }
            let rows = w.actionRows.compactMap { $0 as? FieldRow }.filter { !$0.loadMore }
            guard !rows.isEmpty else { return }
            let n = rows.count, what = n == 1 ? (rows[0].fields["key"] ?? "1 row") : "\(n) rows"
            var items: [(title: String, detail: String)] = [
                ("Copy to clipboard", "\(what) · \(copyKeys.joined(separator: ", "))")]
            let site = cmd.name == "jira" ? jiraSite : ""
            let keyed = rows.filter { !($0.fields["key"] ?? "").isEmpty }
            let keys = keyed.compactMap { $0.fields["key"] }
            let s = keys.count == 1 ? "" : "s"
            if !site.isEmpty && !keys.isEmpty {
                items.append(("Copy URL and title", "\(keys.count) issue\(s) · one “URL Title” line each"))
                items.append(("Open all in browser", "opens \(keys.count) issue\(s) · copies KEY + URL"))
            }
            w.showActionPicker(title: "Actions for \(what)", items: items) { [weak self, weak w] i in
                guard let self, let w, items.indices.contains(i) else { return }
                switch items[i].title {
                case "Copy to clipboard":
                    let text = w.onCopyRows?(rows) ?? ""
                    self.copy(text, "\(n) row(s)")
                    w.showToast("Copied \(what)", symbol: "doc.on.clipboard")
                case "Copy URL and title":
                    let lines = keyed.map { r -> String in
                        let t = (r.fields["title"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                        return "\(site)/browse/\(r.fields["key"] ?? "")" + (t.isEmpty ? "" : " \(t)")
                    }
                    self.copy(lines.joined(separator: "\n"), "\(keys.count) jira URL(s) + titles")
                    w.showToast("Copied \(keys.count) URL\(s) + title\(s)", symbol: "link")
                default:
                    let lines = keys.map { "\($0)\t\(site)/browse/\($0)" }
                    for k in keys {
                        if let u = URL(string: site + "/browse/" + k) { NSWorkspace.shared.open(u) }
                    }
                    self.copy(lines.joined(separator: "\n"), "\(keys.count) jira key(s) + URLs")
                    w.showToast("Opened \(keys.count) · copied keys + URLs", symbol: "safari")
                    self.log("list '\(cmd.name)': opened \(keys.joined(separator: ","))")
                }
            }
        }
        // Cmd+F (jira): the live-search panel docked to this window
        if cmd.name == "jira" {
            w.onCommandF = { [weak self, weak w] in
                guard let self, let w else { return }
                JiraSearchPanel.toggle(on: w, controller: self)
            }
        }
        // clicking the drag header copies the active tab's source path
        w.onChromeHeaderClick = { [weak self] in
            guard let self, tabs.indices.contains(currentTab) else { return }
            self.copy(tabs[currentTab].path, "source path: \(tabs[currentTab].path)")
        }
        // header "config" button: copy the commands.conf path (not on jira:
        // its config lives in the Jira Config window)
        if cmd.name == "jira" { w.copyConfigButtonLabel = "" }
        w.onChromeConfigClick = { [weak self] in
            self?.copy(settings.commandsConfPath, "config path: \(settings.commandsConfPath)")
        }
        let cap = cmd.maxRows > 0 ? cmd.maxRows : Int.max
        var visibleOffset = 0
        var reloadWatcher: Timer?
        // the copy-path / copy-config actions live in the top-left icon menu
        // (below), not as header buttons — keep the header bar uncluttered
        func refreshPathLabel() {
            w.copyPathButtonLabel = ""
        }
        // top-left app glyph: window menu (copy paths, open config, jira
        // poll options, window settings) — same idea as the notes window
        w.onChromeIconClick = { [weak self, weak w] in
            guard let self, let w else { return }
            let menu = NSMenu()
            menu.autoenablesItems = false
            let fm = FileManager.default
            if cmd.name == "jira" {
                // jira: config, paths, jobs, queries, curls, columns — all
                // live in the Jira Config window; this menu is window chrome
                menu.addItem(self.menuItem("Search Jira…  ⌘F") { [weak self, weak w] in
                    guard let self, let w else { return }
                    JiraSearchPanel.toggle(on: w, controller: self)
                })
                menu.addItem(self.menuItem("Open Jira Config Window") { [weak self] in
                    self?.showJiraDashboard()
                })
            } else {
                if tabs.indices.contains(currentTab) {
                    let src = tabs[currentTab].path
                    menu.addItem(self.menuItem("Copy \(URL(fileURLWithPath: src).lastPathComponent) Path") { [weak self] in
                        self?.copy(src, "source path: \(src)")
                    })
                }
                menu.addItem(self.menuItem("Copy Config Path") { [weak self] in
                    self?.copy(settings.commandsConfPath, "config path: \(settings.commandsConfPath)")
                })
                menu.addItem(.separator())
                menu.addItem(self.menuItem("Open Config") { [weak self] in
                    let canonical = NSHomeDirectory() + "/.config/workspace-switcher/commands.conf"
                    let p = fm.fileExists(atPath: canonical) ? canonical : settings.commandsConfPath
                    if fm.fileExists(atPath: p) { self?.openNoteFile(p) }
                })
            }
            menu.addItem(.separator())
            menu.addItem(self.focusLossMenuItem(for: w, section: cmd.name))
            menu.addItem(self.floatMenuItem(for: w, section: cmd.name))
            menu.addItem(.separator())
            self.addThemeMenus(to: menu, window: w, section: cmd.name)
            menu.addItem(.separator())
            menu.addItem(self.menuItem("Reset Default Size") { w.resetToDefaultSize() })
            menu.addItem(self.menuItem("Reset Default Colors") { [weak self] in
                self?.resetWindowTheme(w, section: cmd.name)
            })
            if cmd.name == "jira" {
                menu.addItem(.separator())
                menu.addItem(self.menuItem("Disable Jira…") { [weak self] in
                    self?.disableJiraAsking()
                })
            }
            w.showHeaderMenu(menu)
        }

        // combined filter: search (fuzzy) + dropdown selections, then the
        // table's header sort (numeric-aware; blanks always last)
        func filteredRows(query: String) -> [FieldRow] {
            let byQuery = PopupFuzzy.filter(currentItems(), query: query) { $0.searchText }
            var matched: [FieldRow]
            if activeDims.isEmpty {
                matched = byQuery
            } else {
                let opts = w.filterValues
                matched = byQuery.filter { row in
                    for (i, sel) in w.filterSelections.enumerated() where sel > 0 {
                        guard i < activeDims.count, opts.indices.contains(i),
                              opts[i].indices.contains(sel) else { continue }
                        if row.fields[activeDims[i]] != opts[i][sel] { return false }
                    }
                    return true
                }
            }
            if let k = sortKey {
                matched = matched.enumerated().sorted { a, b in
                    let x = a.element.fields[k.field] ?? "", y = b.element.fields[k.field] ?? ""
                    if x.isEmpty != y.isEmpty { return y.isEmpty }
                    let c = x.localizedStandardCompare(y)
                    if c == .orderedSame { return a.offset < b.offset }
                    return k.ascending ? c == .orderedAscending : c == .orderedDescending
                }.map { $0.element }
            }
            let result = Array(matched.prefix(cap))
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
            if cmd.table {
                let cols = tabColumns(tabs[index].path)
                if cols.map(\.field) != columns.map(\.field) || cols.map(\.width) != columns.map(\.width)
                    || cols.map(\.title) != columns.map(\.title) {
                    columns = cols
                    w.setTableColumns(cols.map { $0.popup })
                }
                if let k = sortKey, !columns.contains(where: { $0.field == k.field }) { sortKey = nil }
                syncSortArrow()
            }
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
        // table header: click = sort (again = flip), divider drag = resize;
        // both persist to commands.conf so the window reopens the same way
        w.onTableSort = { [weak self] i in
            guard let self, columns.indices.contains(i) else { return }
            let f = columns[i].field
            sortKey = sortKey?.field == f ? (f, !(sortKey?.ascending ?? true)) : (f, true)
            syncSortArrow()
            visibleOffset = 0
            w.setRows(filteredRows(query: w.currentQuery), resetScroll: false)
            let v = "\(f):\(sortKey!.ascending ? "asc" : "desc")"
            if let ci = self.commands.firstIndex(where: { $0.name == cmd.name }) {
                self.commands[ci].tableSort = v
            }
            saveConfigValue(section: cmd.name, key: "table-sort", value: v)
            self.log("list '\(cmd.name)': sort -> \(v)")
        }
        // persisted on a short debounce after the LAST live drag update (not
        // only on mouseUp — the header's mouseUp isn't guaranteed to arrive)
        var resizeSave: DispatchWorkItem?
        w.onTableColumnsResized = { [weak self] pcts, final in
            guard let self else { return }
            for i in columns.indices where i < pcts.count { columns[i].width = pcts[i] }
            resizeSave?.cancel()
            let item = DispatchWorkItem { [weak self] in
                guard let self else { return }
                let spec = ListColumn.serialize(columns, titles: cmd.name != "jira")
                // a jira tab owned by a poll job / search saves into THAT job
                if cmd.name == "jira", tabs.indices.contains(currentTab),
                   let own = JiraPoll.owner(ofTab: tabs[currentTab].path) {
                    JiraPoll.run("jira_config.py", ["--set-columns", own.kind, own.name, spec]) { [weak self] code, _, err in
                        self?.log("jira: \(own.kind) \(own.name) columns -> \(spec) (exit \(code))"
                                  + (code == 0 ? "" : " " + err))
                    }
                    return
                }
                if let ci = self.commands.firstIndex(where: { $0.name == cmd.name }) {
                    self.commands[ci].columns = columns
                }
                saveConfigValue(section: cmd.name, key: "columns", value: spec)
                self.log("list '\(cmd.name)': columns -> \(spec)")
            }
            resizeSave = item
            DispatchQueue.main.asyncAfter(deadline: .now() + (final ? 0.05 : 0.6), execute: item)
        }
        w.onEscape = { w.hide(restore: true) }
        w.onHide = { [weak self] restore in
            guard let self else { return }
            reloadWatcher?.invalidate()
            reloadWatcher = nil
            if cmd.name == "jira" { JiraSearchPanel.detach(from: w) }
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
                tabs[i].items = loadListItems(tabs[i].path, cmd: cmd, columns: tabColumns(tabs[i].path))
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
        w.copyConfigButtonLabel = ""
        refreshPathLabel()
        subWindows.append(w)
        w.tabFooterText = lastWriteLabel(tabs[currentTab].path)
        w.show()
        guard cmd.name == "jira" else { return }
        // live search results: reload that tab from disk and select it (a
        // tab the window doesn't have yet = rebuild the window, then select)
        jiraShowTab = { [weak self, weak w] file in
            guard let self, let w else { return }
            guard let i = tabs.firstIndex(where: { ($0.path as NSString).lastPathComponent == file }) else {
                self.pendingJiraTab = file
                self.reloadJiraWindow()
                return
            }
            tabs[i].items = loadListItems(tabs[i].path, cmd: cmd, columns: tabColumns(tabs[i].path))
            tabMtimes[i] = mtime(of: tabs[i].path)
            if i != currentTab {
                w.selectedTab = i
            } else {
                applyFilterData()
                visibleOffset = 0
                w.setRows(filteredRows(query: w.currentQuery))
            }
            w.tabFooterText = lastWriteLabel(tabs[i].path)
        }
        if let f = pendingJiraTab {
            pendingJiraTab = nil
            jiraShowTab?(f)
        }
        JiraSearchPanel.reattach(to: w)
    }

    // Static favorite dirs + zoxide top-N for the file browser, taken from the
    // [files] command section so the notes drawer and the floating window share
    // the same config. Zoxide top-N requires `zoxide` on PATH.
    // [files] browser settings onto a window config (both the notes drawer
    // and the standalone files window read the [files] section)
    private func applyBrowserSettings(_ cfg: inout PopupConfig) {
        guard let cmd = commands.first(where: { $0.kind == .files }) else { return }
        if let v = cmd.sort { cfg.browserSort = v }
        if let v = cmd.sortDescending { cfg.browserSortDescending = v }
        if let v = cmd.searchLimit, v > 0 { cfg.browserSearchLimit = v }
        if let v = cmd.searchExclude { cfg.browserSearchExcludes = v }
        if let v = cmd.terminalWords, !v.isEmpty { cfg.browserTerminalWords = v }
    }
    // sort picked in either browser -> [files] sort / sort-order (and the
    // in-memory spec, so the next browser built opens with it)
    private func saveBrowserSort(_ key: String, _ desc: Bool) {
        let section = commands.first(where: { $0.kind == .files })?.name ?? "files"
        if let i = commands.firstIndex(where: { $0.kind == .files }) {
            commands[i].sort = key
            commands[i].sortDescending = desc
        }
        saveConfigValues(section: section, [("sort", key), ("sort-order", desc ? "desc" : "asc")])
    }
    // `term` in a browser without a shell drawer: [app] terminal-app (default
    // Ghostty when installed, else Terminal) opened on the folder
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
        cfg.floating = cmd.float ?? settings.float
        cfg.escCloseCount = max(0, cmd.escClose ?? settings.escClose)
        cfg.copyToast = settings.copyToast
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
                                 text: cmd.textColor ?? TEXT, dim: cmd.dimColor ?? DIM,
                                 highlight: cmd.highlightColor ?? GROUP_BG,
                                 accent: cmd.accentColor ?? ACCENT)
        cfg.terminalForeground = cmd.terminalForeground
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
        // Top-left icon opens a dropdown menu (color picker, reset, config)
        // — no scattered header buttons.
        w.onChromeIconClick = { [weak self, weak w] in
            guard let self, let w else { return }
            let menu = NSMenu()
            menu.autoenablesItems = false

            menu.addItem(self.focusLossMenuItem(for: w, section: cmd.name))
            menu.addItem(self.floatMenuItem(for: w, section: cmd.name))
            menu.addItem(.separator())
            // theme presets + transparency
            self.addThemeMenus(to: menu, window: w, section: cmd.name)
            menu.addItem(.separator())

            // reset actions
            let resetSizeItem = NSMenuItem(title: "Reset Default Size", action: nil, keyEquivalent: "")
            let rsTarget = MenuActionTarget { w.resetToDefaultSize() }
            resetSizeItem.target = rsTarget
            resetSizeItem.action = #selector(MenuActionTarget.run)
            menuActionTargets.append(rsTarget)
            menu.addItem(resetSizeItem)

            let resetColorItem = NSMenuItem(title: "Reset Default Colors", action: nil, keyEquivalent: "")
            let rcTarget = MenuActionTarget { self.resetWindowTheme(w, section: cmd.name) }
            resetColorItem.target = rcTarget
            resetColorItem.action = #selector(MenuActionTarget.run)
            menuActionTargets.append(rcTarget)
            menu.addItem(resetColorItem)
            menu.addItem(.separator())

            // open config
            let configItem = NSMenuItem(title: "Open Config", action: nil, keyEquivalent: "")
            let cfgTarget = MenuActionTarget {
                let canonical = NSHomeDirectory() + "/.config/workspace-switcher/commands.conf"
                let p = FileManager.default.fileExists(atPath: canonical)
                    ? canonical : settings.commandsConfPath
                if FileManager.default.fileExists(atPath: p) {
                    // open config in the notes window
                    self.openNoteFile(p)
                }
            }
            configItem.target = cfgTarget
            configItem.action = #selector(MenuActionTarget.run)
            menuActionTargets.append(cfgTarget)
            menu.addItem(configItem)

            w.showHeaderMenu(menu)
        }

        let favs = fileBrowserFavoritesConfig()
        var browserCfg = cfg
        applyBrowserSettings(&browserCfg)
        let fb = PopupFileBrowser(config: browserCfg, startDir: root,
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
        fb.onSortChange = { [weak self] key, desc in self?.saveBrowserSort(key, desc) }
        w.onOpenExternalTerminal = { [weak self] dir in self?.openInTerminalApp(dir) }
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
        // table mode: every `filter`-flagged column is searchable too
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
                        .compactMap { (d[$0] as? String).map(compactTimestamp) }
                        .filter { !$0.isEmpty }
                    return vals.isEmpty ? nil : vals.joined(separator: " · ")
                }
                // trailing follows the same rule (e.g. status,priority)
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
    private var servicesHandler: ServicesHandler?

    init(showOnLaunch: Bool, openCommand: String? = nil) {
        self.showOnLaunch = showOnLaunch
        self.openCommand = openCommand
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        installMainMenu()
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
        MenuTarget.controller = c
        installStatusMenus(c)
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

    // One unified menu bar icon with a comprehensive dropdown — replaces the
    // old per-window glyphs. All toggles, resets, and window opens live here.
    private func installStatusMenus(_ c: SwitcherController) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = utilityMenuGlyph
        item.button?.toolTip = "workspace-switcher"
        let menu = NSMenu()
        menu.delegate = MenuTarget.shared  // for checkmark updates
        // shown only while commands.conf has validation findings
        let issues = NSMenuItem(title: "Config Issues…",
                                action: #selector(MenuTarget.showConfigIssues(_:)),
                                keyEquivalent: "")
        issues.target = MenuTarget.shared
        issues.tag = MenuTarget.configIssuesTag
        issues.isHidden = configIssues.isEmpty
        menu.addItem(issues)
        menu.addItem(.separator())

        // File-like section: resets and close
        addMenuItem(menu, "Reset Default Size", #selector(MenuTarget.resetWindowSize(_:)), key: "0", modifiers: .command)
        addMenuItem(menu, "Reset Default Colors", #selector(MenuTarget.resetWindowColors(_:)), key: "")
        menu.addItem(.separator())
        addMenuItem(menu, "Close Window", #selector(MenuTarget.closeWindow(_:)), key: "w", modifiers: .command)
        addMenuItem(menu, "Quit", #selector(MenuTarget.quitApp(_:)), key: "q", modifiers: .command)
        menu.addItem(.separator())

        // Drawer toggles (affect the key/focused window)
        addMenuItem(menu, "Toggle Terminal", #selector(MenuTarget.toggleTerminal(_:)), key: "t", modifiers: [.command, .option])
        addMenuItem(menu, "Toggle File Browser", #selector(MenuTarget.toggleFileBrowser(_:)), key: "b", modifiers: [.command, .option])
        menu.addItem(.separator())

        // Window toggles
        addMenuItem(menu, "Toggle Notes", #selector(MenuTarget.toggleNotes(_:)), key: "n", modifiers: .command)
        addMenuItem(menu, "Toggle Health Checks", #selector(MenuTarget.toggleHealthChecks(_:)), key: "h", modifiers: .command)
        menu.addItem(.separator())

        // Jira: THE SWITCH ([jira] enabled — window + launchd agent; titled
        // "Enable Jira" / "Disable Jira" by menuNeedsUpdate), the window
        // toggle (hidden while disabled, so enabling brings it back without a
        // relaunch), and ONE entry for everything else: the Jira Config window
        // (poll jobs, schedules, JQL, curls, columns, connection)
        addMenuItem(menu, "Enable Jira", #selector(MenuTarget.toggleJiraPoll(_:)), key: "")
        addMenuItem(menu, "Toggle Jira Window", #selector(MenuTarget.toggleJira(_:)), key: "j", modifiers: .command)
        addMenuItem(menu, "Open Jira Config Window", #selector(MenuTarget.openJiraDashboard(_:)), key: "")
        menu.addItem(.separator())

        // Settings submenu with toggleable config options
        let settingsMenu = NSMenu(title: "Settings")
        let settingsItem = NSMenuItem(title: "Settings", action: nil, keyEquivalent: "")
        settingsItem.submenu = settingsMenu
        menu.addItem(settingsItem)

        addMenuItem(settingsMenu, "Hide on Focus Loss", #selector(MenuTarget.toggleHideOnFocusLoss(_:)), key: "")
        addMenuItem(settingsMenu, "Float Windows Above Others", #selector(MenuTarget.toggleFloat(_:)), key: "")
        // Vim Mode toggle — only when a note command is configured
        if c.commands.contains(where: { $0.kind == .note }) {
            addMenuItem(settingsMenu, "Vim Mode (Notes)", #selector(MenuTarget.toggleVimMode(_:)), key: "")
            // Font ▸ / Notes ▸ rebuild on every open (current checkmarks,
            // freshly installed fonts)
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
        configLog("app terminating (front=\(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"))")
    }

    // Build a real macOS app menu (top-left click) so the user has obvious
    // window commands: reset size, close, quit. The accessory policy still
    // hides the menu bar until the app icon is clicked.
    private func installMainMenu() {
        let mainMenu = NSMenu()
        // App menu (appears under "workspace-switcher" when clicked)
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "About workspace-switcher", action: nil, keyEquivalent: "")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "Hide", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
        appMenu.addItem(withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        mainMenu.addItem(appItem)

        // File menu
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

        // Edit menu (standard shortcuts)
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

        // Window menu
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

// Shared target for app menu / status menu actions. Holds a weak reference to
// the running controller so every menu item can reach the app's windows and
// toggle logic. Also serves as NSMenuDelegate to update checkmarks before the
// menu opens (terminal / file browser state of the key window).
final class MenuTarget: NSObject, NSMenuDelegate {
    static let shared = MenuTarget()
    static weak var controller: SwitcherController?

    // MARK: NSMenuDelegate — update checkmarks before the menu opens
    static let configIssuesTag = 7401

    func menuNeedsUpdate(_ menu: NSMenu) {
        if let item = menu.item(withTag: MenuTarget.configIssuesTag) {
            _ = readConfigText()   // re-validates only when the file changed
            item.isHidden = configIssues.isEmpty
            item.title = configUsingBackup
                ? "⚠ Config Invalid — Using Backup…"
                : "⚠ Config Warnings (\(configIssues.count))…"
        }
        guard let controller = MenuTarget.controller else { return }
        // Find the key PopupWindow (the one currently focused)
        let keyWindow = NSApp.keyWindow
        var keyPopup: PopupWindow?
        if let pw = keyWindow?.delegate as? PopupWindow {
            keyPopup = pw
        } else {
            // Fallback: find the first visible PopupWindow
            keyPopup = controller.subWindows.first(where: { $0.isShown })
        }

        for item in menu.items {
            switch item.action {
            case #selector(toggleTerminal(_:)):
                item.state = (keyPopup?.terminalShown ?? false) ? .on : .off
            case #selector(toggleFileBrowser(_:)):
                item.state = (keyPopup?.fileBrowserShown ?? false) ? .on : .off
            case #selector(toggleNotes(_:)):
                item.state = windowState(for: "notes", controller: controller)
            case #selector(toggleJira(_:)):
                item.state = windowState(for: "jira", controller: controller)
                item.isHidden = !jiraEnabledInConfig()
            case #selector(toggleJiraPoll(_:)):
                item.title = jiraEnabledInConfig() ? "Disable Jira" : "Enable Jira"
            case #selector(toggleHealthChecks(_:)):
                item.state = windowState(for: "health-checks", controller: controller)
            case #selector(toggleHideOnFocusLoss(_:)):
                item.state = settings.hideOnFocusLoss ? .on : .off
            case #selector(toggleVimMode(_:)):
                item.state = vimModeEnabled ? .on : .off
            default:
                break
            }
        }
        // Also update submenu items (Settings submenu)
        for item in menu.items {
            if let submenu = item.submenu {
                for subItem in submenu.items {
                    switch subItem.action {
                    case #selector(toggleHideOnFocusLoss(_:)):
                        subItem.state = settings.hideOnFocusLoss ? .on : .off
                    case #selector(toggleFloat(_:)):
                        subItem.state = settings.float ? .on : .off
                    case #selector(toggleVimMode(_:)):
                        subItem.state = vimModeEnabled ? .on : .off
                    default:
                        break
                    }
                }
            }
        }
    }

    // Returns .on if a named window is currently shown, .off otherwise
    private func windowState(for name: String, controller: SwitcherController) -> NSControl.StateValue {
        if let w = controller.subWindows.first(where: { $0.config.name == name }) {
            return w.isShown ? .on : .off
        }
        return .off
    }

    // MARK: File actions

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

    // MARK: Drawer toggles (affect the key/focused window)

    @objc func toggleTerminal(_ sender: Any?) {
        if let pw = keyPopupWindow() {
            pw.toggleTerminalDrawer()
            // Update the header button state to match
            // (header button id 10 = terminal toggle)
            pw.setHeaderButtonOn(10, pw.terminalShown)
        }
    }

    @objc func toggleFileBrowser(_ sender: Any?) {
        if let pw = keyPopupWindow() {
            pw.toggleFileBrowser()
            // Update the header button state to match
            // (header button id 20 = file browser toggle)
            pw.setHeaderButtonOn(20, pw.fileBrowserShown)
        }
    }

    // MARK: Window toggles

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

    @objc func toggleHealthChecks(_ sender: Any?) {
        MenuTarget.controller?.toggleCommand("health-checks")
    }

    // Helper: find the key PopupWindow (the one currently focused)
    private func keyPopupWindow() -> PopupWindow? {
        if let pw = NSApp.keyWindow?.delegate as? PopupWindow {
            return pw
        }
        // Fallback: find the first shown PopupWindow
        return MenuTarget.controller?.subWindows.first(where: { $0.isShown })
    }

    // Whether vim mode is enabled for the notes command
    private var vimModeEnabled: Bool {
        guard let controller = MenuTarget.controller else { return false }
        return controller.commands.first(where: { $0.name == "notes" })?.vimMode ?? false
    }

    // MARK: Config validation

    @objc func showConfigIssues(_ sender: Any?) {
        _ = readConfigText()
        let alert = NSAlert()
        alert.alertStyle = configUsingBackup ? .critical : .warning
        alert.messageText = configUsingBackup
            ? "commands.conf is invalid — the last good backup is in use"
            : "commands.conf has \(configIssues.count) warning\(configIssues.count == 1 ? "" : "s")"
        let lines = configIssues.prefix(15).map { i in
            (i.line > 0 ? "line \(i.line): " : "") + i.message
        }
        let more = configIssues.count > 15 ? "\n…and \(configIssues.count - 15) more" : ""
        alert.informativeText = lines.joined(separator: "\n") + more
            + (configUsingBackup
               ? "\n\nBackup: \(configBackupPath)\nRestoring it moves the invalid file aside as commands.conf.broken-<time>."
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

    // MARK: Settings toggles

    @objc func toggleHideOnFocusLoss(_ sender: Any?) {
        settings.hideOnFocusLoss.toggle()
        saveConfigValue(section: "app", key: "hide-on-focus-loss", value: settings.hideOnFocusLoss ? "true" : "false")
    }

    @objc func toggleFloat(_ sender: Any?) {
        MenuTarget.controller?.setGlobalFloat(!settings.float)
    }

    @objc func toggleVimMode(_ sender: Any?) {
        MenuTarget.controller?.toggleVimModeForNotes()
    }

    @objc func resetSettings(_ sender: Any?) {
        // Reset to defaults: remove custom values from commands.conf
        settings.hideOnFocusLoss = true
        removeConfigValue(section: "app", key: "hide-on-focus-loss")
        MenuTarget.controller?.setGlobalFloat(true)
        removeConfigValue(section: "app", key: "float")
        removeConfigValue(section: "notes", key: "vim-mode")
        removeConfigValue(section: "notes", key: "vim-bin")
    }
}

// MARK: - Font + notes settings menus

// Rebuilds its menu every time it opens (status-bar Settings submenus), so
// checkmarks and newly installed fonts are always current.
final class DynamicMenuDelegate: NSObject, NSMenuDelegate {
    private let build: (NSMenu) -> Void
    init(_ build: @escaping (NSMenu) -> Void) { self.build = build }
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        build(menu)
    }
}
var dynamicMenuDelegates: [DynamicMenuDelegate] = []

// installed font families by type (nerd / mono / sans / serif / display),
// computed once and refreshed after a font install
private var fontFamilyCache: [String: [String]]?
// brew casks currently installing (menu shows "Installing…")
private var fontInstallsRunning: Set<String> = []

enum FontTarget { case editor, terminal }

extension SwitcherController {
    // index of the notes (type = note) command
    var noteCommandIndex: Int? { commands.firstIndex(where: { $0.kind == .note }) }

    // the live notes window, if one exists
    var noteWindow: PopupWindow? {
        guard let i = noteCommandIndex else { return nil }
        return subWindows.first(where: { $0.config.name == commands[i].windowName })
    }

    // closure-backed menu item (targets retained in menuActionTargets)
    func menuItem(_ title: String, state: Bool? = nil, enabled: Bool = true,
                  _ action: @escaping () -> Void) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: #selector(MenuActionTarget.run),
                              keyEquivalent: "")
        let t = MenuActionTarget(action: action)
        item.target = t
        menuActionTargets.append(t)
        if let state { item.state = state ? .on : .off }
        item.isEnabled = enabled
        return item
    }

    // a disabled section label inside a menu
    private func menuHeader(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    // MARK: Font classification

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
        // NSFontFamilyClass lives in the top 4 bits of the symbolic traits
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

    // MARK: Applying fonts

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

    // persist to commands.conf + apply live to every open window
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

    // Cmd+Opt+= / Cmd+Opt+- in the notes window: step both sizes together
    func stepFontSizes(_ delta: Int) {
        applyFontSize(currentFontSize(.editor) + CGFloat(delta), target: .editor)
        applyFontSize(currentFontSize(.terminal) + CGFloat(delta), target: .terminal)
    }

    // MARK: Font menu

    // Font ▸ Editor Font ▸ <type> ▸ families, Terminal Font ▸ …, Size ▸ …,
    // Install Font ▸ <type> ▸ casks, Other… (system font panel)
    func buildFontMenu(into menu: NSMenu) {
        let byType = SwitcherController.installedFontsByType()
        for (target, title) in [(FontTarget.editor, "Editor Font"),
                                (FontTarget.terminal, "Terminal Font")] {
            let current = currentFont(target)
            let sub = NSMenu(title: title)
            // the terminal (and the vim pane) need a fixed-pitch grid
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
        // Install Font ▸ grouped by type (curated brew casks, commands.conf
        // `font-install-casks`)
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

    // loose match: "JetBrains Mono Nerd Font" vs family "JetBrainsMono Nerd Font"
    static func isFontInstalled(_ label: String) -> Bool {
        let norm = { (s: String) in s.lowercased().filter { $0.isLetter || $0.isNumber } }
        let want = norm(label)
        return NSFontManager.shared.availableFontFamilies.contains { norm($0) == want }
    }

    // brew install --cask <cask> in the background; on success offer to use
    // the new font right away
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
        let p = Process()
        p.executableURL = URL(fileURLWithPath: brew)
        p.arguments = ["install", "--cask", cask]
        var env = ProcessInfo.processInfo.environment
        env["HOMEBREW_NO_AUTO_UPDATE"] = "1"
        env["HOMEBREW_NO_INSTALL_CLEANUP"] = "1"
        p.environment = env
        let errPipe = Pipe()
        p.standardError = errPipe
        p.standardOutput = FileHandle.nullDevice
        p.terminationHandler = { proc in
            let err = String(data: errPipe.fileHandleForReading.readDataToEndOfFile(),
                             encoding: .utf8) ?? ""
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                fontInstallsRunning.remove(cask)
                fontFamilyCache = nil
                guard proc.terminationStatus == 0 else {
                    let msg = err.split(separator: "\n").last.map(String.init) ?? "exit \(proc.terminationStatus)"
                    self.log("font install \(cask) failed: \(err)")
                    self.noteWindow?.setStatus("Install failed: \(msg)", isError: true)
                    return
                }
                self.log("font install \(cask): ok")
                self.noteWindow?.setStatus(nil, isError: false)
                self.offerNewFont(label: label, before: before, tries: 0)
            }
        }
        do { try p.run() } catch {
            fontInstallsRunning.remove(cask)
            log("font install \(cask): \(error)")
            noteWindow?.setStatus("Install failed: \(error.localizedDescription)", isError: true)
        }
    }

    // the system registers new font files a moment after they land in
    // ~/Library/Fonts — poll briefly for the new family, then ask where to
    // use it
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

    // the system Font Panel for anything not in the lists; picks apply to
    // the editor font
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

    // MARK: Notes settings menu

    func buildNotesSettingsMenu(into menu: NSMenu) {
        guard let i = noteCommandIndex else { return }
        let cmd = commands[i]
        menu.addItem(menuItem("Vim Mode", state: cmd.vimMode) { [weak self] in
            self?.toggleVimModeForNotes()
        })
        // editor binary: only the ones actually installed
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
        // drawer open at launch
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
        menu.addItem(menuItem("Keep Visible When Unfocused (Sticky)", state: cmd.sticky) { [weak self] in
            let v = !cmd.sticky
            self?.updateNoteSetting("sticky", v ? "true" : "false", rebuild: false) { $0.sticky = v }
            self?.noteWindow?.config.sticky = v
        })
        menu.addItem(.separator())
        let firstDir = cmd.paths.first.map { p -> String in
            let e = (p as NSString).expandingTildeInPath
            var isDir: ObjCBool = false
            FileManager.default.fileExists(atPath: e, isDirectory: &isDir)
            return isDir.boolValue ? e : (e as NSString).deletingLastPathComponent
        }
        if let dir = firstDir {
            menu.addItem(menuItem("Open Notes Folder in Finder") {
                NSWorkspace.shared.open(URL(fileURLWithPath: dir))
            })
        }
        menu.addItem(menuItem("Reload Config") { [weak self] in
            self?.reloadConfig()
        })
    }

    // persist one [notes] key, update the in-memory spec, and (optionally)
    // rebuild the open notes window so it takes effect now
    func updateNoteSetting(_ key: String, _ value: String, rebuild: Bool,
                           _ mutate: (inout CommandSpec) -> Void) {
        guard let i = noteCommandIndex else { return }
        mutate(&commands[i])
        saveConfigValue(section: commands[i].name, key: key, value: value)
        log("notes setting: \(key) = \(value)")
        if rebuild { rebuildNoteWindow() }
    }

    // tear down + reopen the notes window with the current spec (the note is
    // flushed first: vim :wall, native saveNote via onEditorClose)
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
            openNoteWindow(commands[i], restoreWID: restoreWID, restorePID: restorePID)
        }
    }

    // re-read commands.conf and rebuild the notes window from it
    func reloadConfig() {
        commands = loadCommands()
        fontFamilyCache = nil
        log("config reloaded (\(commands.count) commands)")
        rebuildNoteWindow()
    }
}

// NSFontManager target for the system Font Panel ("Other Font…")
final class FontPanelReceiver: NSObject {
    static let shared = FontPanelReceiver()
    var onPick: ((String) -> Void)?
    @objc func changeFont(_ sender: Any?) {
        guard let fm = sender as? NSFontManager else { return }
        let f = fm.convert(NSFont.systemFont(ofSize: 13))
        if let fam = f.familyName { onPick?(fam) }
    }
}

// MARK: - Jira poll (menu-bar switch, polling options, setup window)

// The python poller (jira/jira_poll.py + friends) owns the network, the cache
// and ~/.cache/jira/status.json; the app only flips THE SWITCH ([jira]
// enabled), kicks polls, edits schedules through jira_config.py, and SHOWS
// the state — so the menu, jira-doctor and `cat status.json` always agree.
enum JiraPoll {
    static var dir: String { binDir + "/jira" }
    static let configPath = NSHomeDirectory() + "/.config/jira/config.json"
    static let statusPath = NSHomeDirectory() + "/.cache/jira/status.json"
    static let curlLogPath = NSHomeDirectory() + "/.cache/jira/curl.log"
    static var pollScript: String { dir + "/jira_poll.py" }
    // why the last menu-bar enable attempt left jira disabled (submenu line)
    static var lastEnableError: String?
    // "Poll Now" jobs in flight (endpoint name, or "all")
    static var running: Set<String> = []
    // schedule choices offered under Poll Interval ▸
    static let intervals = ["5m", "10m", "15m", "30m", "1h", "2h", "4h", "1d", "1w"]
    static let directoryPath = NSHomeDirectory() + "/.cache/jira/directory.json"
    // the live search's tab (jira_config.LIVE_SEARCH_FILE, in outDir)
    static let liveSearchFile = "search.json"

    // a column field's built-in name — mirror of jira_config.BASE_FIELD_LABELS
    static let baseFieldLabels: [String: String] = [
        "key": "Key", "title": "Title", "status": "Status", "assignee": "Assignee",
        "reporter": "Reporter", "priority": "Priority", "labels": "Labels",
        "description": "Description", "project": "Project", "updated": "Updated",
        "release": "Fix versions", "releaseLabel": "Release", "releaseDate": "Release date",
        "releaseStatus": "Released", "comments": "Comments",
    ]

    // every field's ONE label (Jira Config ▸ Definitions ▸ Fields): team.json
    // field_labels, else a custom field's own label, else the built-in name
    // (jira_config.field_label). Column headers everywhere use it.
    static func fieldLabels() -> [String: String] {
        var out = baseFieldLabels
        let teamPath = (readJSON(configPath)?["teamConfig"] as? String).map { ($0 as NSString).expandingTildeInPath }
            ?? NSHomeDirectory() + "/.config/jira/team.json"
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

    // column titles = the fields' labels (a spec title only for a field
    // that has no label, e.g. a raw Jira field id)
    static func labeled(_ cols: [ListColumn]) -> [ListColumn] {
        let labels = fieldLabels()
        return cols.map { c in
            var c = c
            if let l = labels[c.field] { c.title = l }
            return c
        }
    }

    // Run a jira/*.py script off the main thread; `done` gets (exit code,
    // stdout, stderr) on the main thread. stdin carries secrets (the token)
    // so they never show up in `ps`.
    static func run(_ script: String, _ args: [String], stdin: String? = nil,
                    done: ((Int32, String, String) -> Void)? = nil) {
        DispatchQueue.global(qos: .userInitiated).async {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
            p.arguments = ["python3", dir + "/" + script] + args
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:" + (env["PATH"] ?? "")
            p.environment = env
            let out = Pipe(), err = Pipe(), inp = Pipe()
            p.standardOutput = out
            p.standardError = err
            p.standardInput = inp
            do { try p.run() } catch {
                DispatchQueue.main.async { done?(-1, "", "cannot run python3: \(error)") }
                return
            }
            if let s = stdin { inp.fileHandleForWriting.write(Data(s.utf8)) }
            try? inp.fileHandleForWriting.close()
            // drain both pipes concurrently — a full stderr buffer must never
            // block the child while we wait on stdout
            var o = Data(), e = Data()
            let g = DispatchGroup()
            g.enter()
            DispatchQueue.global().async { o = out.fileHandleForReading.readDataToEndOfFile(); g.leave() }
            e = err.fileHandleForReading.readDataToEndOfFile()
            g.wait()
            p.waitUntilExit()
            let code = p.terminationStatus
            DispatchQueue.main.async {
                done?(code, String(decoding: o, as: UTF8.self), String(decoding: e, as: UTF8.self))
            }
        }
    }

    static func readJSON(_ path: String) -> [String: Any]? {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    static var status: [String: Any]? { readJSON(statusPath) }
    static var endpoints: [[String: Any]] { readJSON(configPath)?["endpoints"] as? [[String: Any]] ?? [] }

    // the poll job (or the live search) that writes this jira tab (json
    // file), with its own columns — nil for a file no job owns
    static func owner(ofTab path: String) -> (kind: String, name: String, columns: [ListColumn])? {
        guard let d = readJSON(configPath) else { return nil }
        let file = (path as NSString).lastPathComponent
        if file == liveSearchFile {
            let ls = d["liveSearch"] as? [String: Any] ?? [:]
            return ("live", "search", ListColumn.parse(ls["columns"] as? String))
        }
        for e in d["endpoints"] as? [[String: Any]] ?? [] {
            guard let name = e["name"] as? String, (e["type"] as? String) != "directory" else { continue }
            if (e["file"] as? String ?? "\(name).json") == file {
                return ("endpoint", name, ListColumn.parse(e["columns"] as? String))
            }
        }
        return nil
    }

    // last meaningful stderr line of a failed script ("jira-api: …" prefix off)
    static func errorLine(_ err: String, fallback: String) -> String {
        let line = err.split(separator: "\n").map(String.init)
            .last(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) ?? fallback
        for p in ["jira-api: ", "jira-poll: ", "jira-config: "] where line.hasPrefix(p) {
            return String(line.dropFirst(p.count))
        }
        return line
    }

    // "2026-09-23 22:19:17" -> "22:19" today, "Sep 22 22:19" otherwise
    static func short(_ ts: String?) -> String {
        guard let ts, ts.count >= 16 else { return ts ?? "never" }
        let today = String(ISO8601DateFormatter.string(from: Date(), timeZone: .current,
                                                       formatOptions: [.withFullDate]))
        let hm = String(ts.dropFirst(11).prefix(5))
        return ts.hasPrefix(today) ? hm : String(ts.prefix(10)) + " " + hm
    }
}

extension SwitcherController {
    // menu-bar "Enable Jira" / "Disable Jira": on -> off asks whether the
    // poller should keep running in the background; off -> on checks the
    // config, tests the login, and only THEN flips (setup window when the
    // config is missing, an explained failure when the login fails)
    func toggleJiraPoll() {
        if jiraEnabledInConfig() {
            disableJiraAsking()
        } else {
            enableJiraChecked()
        }
    }

    // Disabling while the poller is loaded: offer to keep it polling in the
    // background (cache stays fresh, window + menu entries hide) or stop it.
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
        // enabling always clears the background-poll flag (enabled implies
        // polling); disabling sets it only when the user chose Keep Polling
        saveConfigValues(section: "jira", [
            ("enabled", on ? "true" : "false"),
            ("poll-when-disabled", !on && keepPolling ? "true" : nil),
        ])
        // reloadConfig -> loadCommands -> syncJiraLaunchAgent: the agent is
        // bootstrapped (RunAtLoad polls at once) or booted out right here
        reloadConfig()
        log("jira: [jira] enabled = \(on)\(!on && keepPolling ? " (background polling kept)" : "") (menu-bar switch)")
        if on {
            JiraPoll.lastEnableError = nil
            JiraPoll.run("jira_status.py", ["--note-error"])
            showCommand("jira")
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

    func showJiraDashboard() {
        JiraDashboardWindow.show(controller: self)
    }

    // "Poll Now": non-blocking; the poll holds its own lock, the dashboard
    // shows "running" until it returns. full = --init (full resync).
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

    // rebuild an open jira window so it picks up new / removed tabs and each
    // tab's current columns (after Jira Config window edits)
    func reloadJiraWindow() {
        guard let w = subWindows.first(where: { $0.config.name == "jira" }) else { return }
        let wasShown = w.isShown
        jiraShowTab = nil
        w.hide(restore: false)
        subWindows.removeAll { $0 === w }
        w.releaseHooks()
        w.nativeWindow.orderOut(nil)
        if wasShown { showCommand("jira") }
    }
}

// Jira credentials window: site, email (Cloud only — blank = Bearer token for
// Server/Data Center), token (secure), default project, max results. "Test
// Connection" runs jira_api.py --myself against what is typed; "Copy curl"
// copies that same request as a runnable curl (jira_api.py --curl); "Save & Enable" writes config.json (chmod 600, via jira_config.py
// --save — the token travels on stdin), re-tests, then flips [jira] enabled.
// A plain titled NSWindow (not an NSAlert) so every field takes focus, and a
// local key monitor routes Cmd/Ctrl+V, Cmd+A/C/X/Z to the field editor
// (rule.md #1 — the accessory app has no reliable Edit key equivalents).
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
    // auth mode the last Test / Save detection found (jira_api.py --detect-auth)
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
        // above the popup windows (they float at .popUpMenu) — otherwise the
        // setup window opens hidden behind the jira window
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
            ("Default project", project, "e.g. SAM1 (optional)"),
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
            // Esc = Cancel even when a field editor swallows the key
            // equivalent (accessory app: no reliable button key equivalents)
            if e.keyCode == 53 { self.close(); return nil }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command), ctrl = mods.contains(.control)
            guard cmd || ctrl, let ed = self.window.firstResponder as? NSText else { return e }
            switch e.keyCode {
            case 9: ed.paste(nil)                        // Cmd+V / Ctrl+V
            case 8: ed.copy(nil)                         // Cmd+C / Ctrl+C
            case 0 where cmd: ed.selectAll(nil)          // Cmd+A
            case 7 where cmd: ed.cut(nil)                // Cmd+X
            case 6 where cmd: ed.undoManager?.undo()     // Cmd+Z
            default: return e
            }
            return nil
        }
    }

    // current config (token never leaves python — only whether one is set)
    private func prefill() {
        JiraPoll.run("jira_config.py", ["--check"]) { [weak self] _, out, _ in
            guard let self,
                  let d = (try? JSONSerialization.jsonObject(with: Data(out.utf8))) as? [String: Any]
            else { return }
            if self.site.stringValue.isEmpty { self.site.stringValue = d["site"] as? String ?? "" }
            // a saved Bearer setup never gets an email pre-filled (e.g. from
            // JIRA_EMAIL) — that would silently switch it to basic auth
            if self.email.stringValue.isEmpty, d["auth"] as? String != "bearer" {
                self.email.stringValue = d["email"] as? String ?? ""
            }
            if self.project.stringValue.isEmpty { self.project.stringValue = d["defaultProject"] as? String ?? "" }
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

    // typed values -> jira_api.py flags (blank site / token fall back to
    // config/env; the email is always the typed one); the token travels on stdin
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

    // jira_api.py --detect-auth on the typed values: tries Bearer and email +
    // token against /myself; done(auth, user) on success, else (nil, error)
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

    @objc private func test(_ sender: Any?) {
        busy(true)
        setResult("Testing (detecting the auth type)…", ok: nil)
        detect { [weak self] auth, msg in
            self?.busy(false)
            guard let auth else { self?.setResult("✗ \(msg)", ok: false); return }
            self?.setResult("✓ Connected as \(msg) — \(Self.authTitle(auth))", ok: true)
        }
    }

    // detect the auth type on the typed values, save it with them, enable
    @objc private func saveAndEnable(_ sender: Any?) {
        let v = trimmed
        guard !v.site.isEmpty, v.site.hasPrefix("http") else {
            setResult("✗ Site URL must start with https://", ok: false)
            return
        }
        busy(true)
        setResult("Detecting the auth type…", ok: nil)
        detect { [weak self] auth, msg in
            guard let self else { return }
            let mode = auth ?? self.detectedAuth ?? (v.email.isEmpty ? "bearer" : "basic")
            var obj: [String: Any] = ["site": v.site, "email": mode == "basic" ? v.email : "",
                                      "auth": mode,
                                      "defaultProject": v.project, "defaultMax": Int(v.max) ?? 25]
            if !v.token.isEmpty { obj["token"] = v.token }
            guard let data = try? JSONSerialization.data(withJSONObject: obj) else { self.busy(false); return }
            self.setResult("Saving…", ok: nil)
            JiraPoll.run("jira_config.py", ["--save"], stdin: String(decoding: data, as: UTF8.self)) {
                [weak self] code, _, err in
                guard let self else { return }
                self.busy(false)
                guard code == 0 else {
                    self.setResult("✗ save failed: \(JiraPoll.errorLine(err, fallback: "exit \(code)"))", ok: false)
                    return
                }
                guard auth != nil else {
                    self.setResult("✗ saved, but login failed: \(msg) — polling stays off", ok: false)
                    JiraPoll.lastEnableError = msg
                    return
                }
                self.setResult("✓ Connected as \(msg) — \(Self.authTitle(mode)) — enabling…", ok: true)
                let c = self.controller
                self.close()
                c?.setJiraEnabled(true)
            }
        }
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
