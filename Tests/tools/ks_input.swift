// ks_input.swift — post real keyboard/mouse events to one named application.
//
// Built by bin/ks-input (swiftc -O) and used by the UI test scripts to drive
// kitchen-sink the way a person does. Everything that SENDS input requires
// `--pid P` and refuses (exit 3) unless P is the frontmost application at the
// moment of *each* event — so a stray run can never type into whatever the
// owner happens to be using. There is no flag that bypasses the check.
//
//   front                                  print "<pid>\t<bundle id>\t<name>" of the frontmost app
//   windows --pid P                        on-screen windows of P as JSON
//   activate --pid P                       make P frontmost (exit 0 only if it is, within 2s)
//   key --pid P <combo> [<combo>...]       send key combos: cmd+k, esc, return, tab, shift+tab,
//                                          down, ctrl+b, cmd+shift+p, f5, space, delete, a, cmd+/
//   type --pid P <text>                    type text (one unicode event per character)
//   click --pid P <x> <y> [--right] [--double]
//   drag --pid P <x1> <y1> <x2> <y2> [--steps N]
//   scroll --pid P <dy>
//   shot --pid P --out <file.png> [--win <id>]
//
// Coordinates are global, top-left origin; click/drag/scroll/shot also take
// `--win <id>` to make them relative to that window's top-left.
//
// Exit codes: 0 ok, 2 usage, 3 refused (not frontmost), 4 window not found, 5 other.
import AppKit
import Carbon.HIToolbox
import CoreGraphics
import Foundation

// MARK: - exit codes / io

let exitOK: Int32 = 0
let exitUsage: Int32 = 2
let exitRefused: Int32 = 3
let exitNoWindow: Int32 = 4
let exitOther: Int32 = 5

func stdoutLine(_ s: String) {
    FileHandle.standardOutput.write(Data((s + "\n").utf8))
}

func stderrLine(_ s: String) {
    FileHandle.standardError.write(Data((s + "\n").utf8))
}

/// A failure that maps to one of the documented exit codes.
struct CliError: Error {
    let code: Int32
    let message: String
}

let usage = """
usage: ks-input <command> [options]

  front                              print "<pid>\\t<bundle id>\\t<name>" of the frontmost app
  windows --pid P                    on-screen windows of P as JSON
  activate --pid P                   make P frontmost (exit 0 only if it is, within 2s)
  key --pid P <combo> [<combo>...]   send key combos (cmd+k, esc, ctrl+b, f5, cmd+/, ...)
  type --pid P <text>                type text, one unicode event per character
  click --pid P <x> <y> [--right] [--double]
  drag --pid P <x1> <y1> <x2> <y2> [--steps N]
  scroll --pid P <dy>
  shot --pid P --out <file.png> [--win <id>]

Coordinates are global, top-left origin. click/drag/scroll/shot also accept
--win <id> to use that window's top-left as the origin.

Every command that sends input checks, before each single event, that the
frontmost application's pid is exactly P; otherwise it refuses with exit 3 and
sends nothing. There is no flag to skip that check.

Text or coordinates that begin with '-' can be passed after a literal '--'.
"""

// MARK: - the safety gate

/// Refuses unless the frontmost application is exactly `pid`.
///
/// This is re-read live (NSWorkspace) immediately before every event that is
/// posted — never cached once per command — so focus stolen mid-run still
/// stops delivery. There is deliberately no bypass.
func requireFrontmost(_ pid: pid_t, _ what: String) throws {
    // NSWorkspace updates frontmostApplication from notifications delivered on
    // the run loop; drain it first or a long `type`/`drag` sees a stale value.
    RunLoop.current.run(mode: .default, before: Date())
    guard let front = NSWorkspace.shared.frontmostApplication else {
        throw CliError(code: exitRefused,
                       message: "refused (\(what)): no frontmost application")
    }
    guard front.processIdentifier == pid else {
        let bundle = front.bundleIdentifier ?? "-"
        throw CliError(code: exitRefused,
                       message: "refused (\(what)): frontmost is pid \(front.processIdentifier) "
                              + "\(front.localizedName ?? "?") [\(bundle)], not pid \(pid)")
    }
}

// MARK: - event sender

/// Posts events to the HID tap after checking the gate for each one.
struct Sender {
    let pid: pid_t
    private let source: CGEventSource?

    init(pid: pid_t) {
        self.pid = pid
        self.source = CGEventSource(stateID: .hidSystemState)
    }

    /// The only place events are posted. Every call re-checks the frontmost
    /// application, so there is no path that sends without the gate.
    func post(_ event: CGEvent, _ what: String) throws {
        try requireFrontmost(pid, what)
        event.post(tap: .cghidEventTap)
    }

    func key(_ code: CGKeyCode, flags: CGEventFlags, _ what: String) throws {
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else {
            throw CliError(code: exitOther, message: "could not create key event")
        }
        down.flags = flags
        up.flags = flags
        try post(down, "\(what) keyDown")
        try post(up, "\(what) keyUp")
    }

    func type(_ character: Character) throws {
        let units = Array(String(character).utf16)
        guard !units.isEmpty,
              let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) else {
            throw CliError(code: exitOther, message: "could not create typing event")
        }
        units.withUnsafeBufferPointer { buf in
            down.keyboardSetUnicodeString(stringLength: buf.count, unicodeString: buf.baseAddress!)
            up.keyboardSetUnicodeString(stringLength: buf.count, unicodeString: buf.baseAddress!)
        }
        try post(down, "type down")
        try post(up, "type up")
    }

    func mouse(_ kind: CGEventType, _ point: CGPoint, _ button: CGMouseButton, _ what: String) throws {
        guard let event = CGEvent(mouseEventSource: source, mouseType: kind,
                                  mouseCursorPosition: point, mouseButton: button) else {
            throw CliError(code: exitOther, message: "could not create mouse event")
        }
        try post(event, what)
    }
}

// MARK: - key codes

/// Virtual key codes for a US layout, taken straight from the SDK header
/// Carbon ▸ HIToolbox ▸ Events.h (the kVK_* enum) instead of being transcribed.
let keyCodes: [String: CGKeyCode] = [
    "a": CGKeyCode(kVK_ANSI_A), "b": CGKeyCode(kVK_ANSI_B), "c": CGKeyCode(kVK_ANSI_C),
    "d": CGKeyCode(kVK_ANSI_D), "e": CGKeyCode(kVK_ANSI_E), "f": CGKeyCode(kVK_ANSI_F),
    "g": CGKeyCode(kVK_ANSI_G), "h": CGKeyCode(kVK_ANSI_H), "i": CGKeyCode(kVK_ANSI_I),
    "j": CGKeyCode(kVK_ANSI_J), "k": CGKeyCode(kVK_ANSI_K), "l": CGKeyCode(kVK_ANSI_L),
    "m": CGKeyCode(kVK_ANSI_M), "n": CGKeyCode(kVK_ANSI_N), "o": CGKeyCode(kVK_ANSI_O),
    "p": CGKeyCode(kVK_ANSI_P), "q": CGKeyCode(kVK_ANSI_Q), "r": CGKeyCode(kVK_ANSI_R),
    "s": CGKeyCode(kVK_ANSI_S), "t": CGKeyCode(kVK_ANSI_T), "u": CGKeyCode(kVK_ANSI_U),
    "v": CGKeyCode(kVK_ANSI_V), "w": CGKeyCode(kVK_ANSI_W), "x": CGKeyCode(kVK_ANSI_X),
    "y": CGKeyCode(kVK_ANSI_Y), "z": CGKeyCode(kVK_ANSI_Z),
    "0": CGKeyCode(kVK_ANSI_0), "1": CGKeyCode(kVK_ANSI_1), "2": CGKeyCode(kVK_ANSI_2),
    "3": CGKeyCode(kVK_ANSI_3), "4": CGKeyCode(kVK_ANSI_4), "5": CGKeyCode(kVK_ANSI_5),
    "6": CGKeyCode(kVK_ANSI_6), "7": CGKeyCode(kVK_ANSI_7), "8": CGKeyCode(kVK_ANSI_8),
    "9": CGKeyCode(kVK_ANSI_9),
    "-": CGKeyCode(kVK_ANSI_Minus), "=": CGKeyCode(kVK_ANSI_Equal),
    "[": CGKeyCode(kVK_ANSI_LeftBracket), "]": CGKeyCode(kVK_ANSI_RightBracket),
    "\\": CGKeyCode(kVK_ANSI_Backslash), ";": CGKeyCode(kVK_ANSI_Semicolon),
    "'": CGKeyCode(kVK_ANSI_Quote), ",": CGKeyCode(kVK_ANSI_Comma),
    ".": CGKeyCode(kVK_ANSI_Period), "/": CGKeyCode(kVK_ANSI_Slash),
    "`": CGKeyCode(kVK_ANSI_Grave),
    "return": CGKeyCode(kVK_Return), "enter": CGKeyCode(kVK_Return),
    "tab": CGKeyCode(kVK_Tab), "space": CGKeyCode(kVK_Space),
    "delete": CGKeyCode(kVK_Delete), "backspace": CGKeyCode(kVK_Delete),
    "forwarddelete": CGKeyCode(kVK_ForwardDelete),
    "esc": CGKeyCode(kVK_Escape), "escape": CGKeyCode(kVK_Escape),
    "left": CGKeyCode(kVK_LeftArrow), "right": CGKeyCode(kVK_RightArrow),
    "up": CGKeyCode(kVK_UpArrow), "down": CGKeyCode(kVK_DownArrow),
    "home": CGKeyCode(kVK_Home), "end": CGKeyCode(kVK_End),
    "pageup": CGKeyCode(kVK_PageUp), "pagedown": CGKeyCode(kVK_PageDown),
    "f1": CGKeyCode(kVK_F1), "f2": CGKeyCode(kVK_F2), "f3": CGKeyCode(kVK_F3),
    "f4": CGKeyCode(kVK_F4), "f5": CGKeyCode(kVK_F5), "f6": CGKeyCode(kVK_F6),
    "f7": CGKeyCode(kVK_F7), "f8": CGKeyCode(kVK_F8), "f9": CGKeyCode(kVK_F9),
    "f10": CGKeyCode(kVK_F10), "f11": CGKeyCode(kVK_F11), "f12": CGKeyCode(kVK_F12),
]

func modifierFlag(_ token: String) -> CGEventFlags? {
    switch token {
    case "cmd", "command", "meta": return .maskCommand
    case "ctrl", "control": return .maskControl
    case "alt", "opt", "option": return .maskAlternate
    case "shift": return .maskShift
    case "fn": return .maskSecondaryFn
    default: return nil
    }
}

func parseCombo(_ combo: String) throws -> (CGKeyCode, CGEventFlags) {
    let parts = combo.lowercased().split(separator: "+").map(String.init)
    guard !parts.isEmpty, !combo.isEmpty else {
        throw CliError(code: exitUsage, message: "empty key combo")
    }
    var flags: CGEventFlags = []
    var code: CGKeyCode?
    for part in parts {
        if let flag = modifierFlag(part) {
            flags.insert(flag)
        } else if let key = keyCodes[part] {
            code = key
        } else {
            throw CliError(code: exitUsage, message: "unknown key '\(part)' in combo '\(combo)'")
        }
    }
    guard let resolved = code else {
        throw CliError(code: exitUsage, message: "combo '\(combo)' has no non-modifier key")
    }
    return (resolved, flags)
}

// MARK: - windows

struct WinInfo {
    let id: CGWindowID
    let title: String
    let layer: Int
    let rect: CGRect
}

/// P's on-screen windows, front-to-back (the order CGWindowList returns).
func windowList(of pid: pid_t) -> [WinInfo] {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return []
    }
    var out: [WinInfo] = []
    for entry in raw {
        let owner = (entry[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value ?? -1
        guard owner == pid else { continue }
        let number = (entry[kCGWindowNumber as String] as? NSNumber)?.uint32Value ?? 0
        let title = entry[kCGWindowName as String] as? String ?? ""
        let layer = (entry[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0
        var rect = CGRect.zero
        if let bounds = entry[kCGWindowBounds as String] as? [String: Any] {
            rect = CGRect(x: (bounds["X"] as? NSNumber)?.doubleValue ?? 0,
                          y: (bounds["Y"] as? NSNumber)?.doubleValue ?? 0,
                          width: (bounds["Width"] as? NSNumber)?.doubleValue ?? 0,
                          height: (bounds["Height"] as? NSNumber)?.doubleValue ?? 0)
        }
        out.append(WinInfo(id: number, title: title, layer: layer, rect: rect))
    }
    return out
}

/// The window P would be showing: the frontmost normal window with real area,
/// else the frontmost window with real area, else the frontmost window.
func frontWindow(of pid: pid_t, in list: [WinInfo]) throws -> WinInfo {
    let sized = list.filter { $0.rect.width > 0 && $0.rect.height > 0 }
    if let normal = sized.first(where: { $0.layer == 0 }) { return normal }
    if let any = sized.first { return any }
    if let first = list.first { return first }
    throw CliError(code: exitNoWindow, message: "pid \(pid) has no on-screen window")
}

func window(of pid: pid_t, id: CGWindowID) throws -> WinInfo {
    guard let found = windowList(of: pid).first(where: { $0.id == id }) else {
        throw CliError(code: exitNoWindow, message: "pid \(pid) has no on-screen window with id \(id)")
    }
    return found
}

// MARK: - argument parsing

struct Args {
    var positionals: [String] = []
    var values: [String: String] = [:]
    var switches: Set<String> = []
}

func parseArgs(_ tokens: [String], valueFlags: Set<String>, switchFlags: Set<String>) throws -> Args {
    var out = Args()
    var i = 0
    var literal = false
    while i < tokens.count {
        let token = tokens[i]
        if literal {
            out.positionals.append(token)
            i += 1
            continue
        }
        if token == "--" {
            literal = true
            i += 1
            continue
        }
        if token.hasPrefix("--") {
            if valueFlags.contains(token) {
                guard i + 1 < tokens.count else {
                    throw CliError(code: exitUsage, message: "\(token) needs a value")
                }
                out.values[token] = tokens[i + 1]
                i += 2
                continue
            }
            if switchFlags.contains(token) {
                out.switches.insert(token)
                i += 1
                continue
            }
            throw CliError(code: exitUsage, message: "unknown option \(token)")
        }
        if token.hasPrefix("-"), token.count > 1 {
            // A leading '-' is only an option if it is not a negative number,
            // so `scroll --pid P -3` and negative coordinates work.
            if let first = token.dropFirst().first, !(first.isNumber || first == ".") {
                throw CliError(code: exitUsage, message: "unknown option \(token)")
            }
        }
        out.positionals.append(token)
        i += 1
    }
    return out
}

func requiredPid(_ args: Args) throws -> pid_t {
    guard let raw = args.values["--pid"] else {
        throw CliError(code: exitUsage, message: "--pid P is required")
    }
    guard let pid = Int32(raw), pid > 0 else {
        throw CliError(code: exitUsage, message: "--pid must be a positive integer (got '\(raw)')")
    }
    return pid
}

func number(_ raw: String, _ what: String) throws -> Double {
    guard let value = Double(raw) else {
        throw CliError(code: exitUsage, message: "\(what) must be a number (got '\(raw)')")
    }
    return value
}

func optionalWindowId(_ args: Args) throws -> CGWindowID? {
    guard let raw = args.values["--win"] else { return nil }
    guard let id = UInt32(raw) else {
        throw CliError(code: exitUsage, message: "--win must be a window id (got '\(raw)')")
    }
    return id
}

/// Origin for relative coordinates: the window's top-left, or (0, 0).
func origin(for pid: pid_t, args: Args) throws -> CGPoint {
    guard let id = try optionalWindowId(args) else { return .zero }
    return try window(of: pid, id: id).rect.origin
}

// MARK: - commands

func cmdFront() -> Int32 {
    let app = NSWorkspace.shared.frontmostApplication
    let pid = app?.processIdentifier ?? 0
    let bundle = app?.bundleIdentifier ?? "-"
    let name = app?.localizedName ?? "-"
    stdoutLine("\(pid)\t\(bundle)\t\(name)")
    return exitOK
}

func cmdWindows(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    let rows: [[String: Any]] = windowList(of: pid).map { win in
        [
            "id": Int(win.id),
            "title": win.title,
            "x": Double(win.rect.origin.x),
            "y": Double(win.rect.origin.y),
            "w": Double(win.rect.size.width),
            "h": Double(win.rect.size.height),
        ]
    }
    guard let data = try? JSONSerialization.data(withJSONObject: rows, options: [.sortedKeys]),
          let text = String(data: data, encoding: .utf8) else {
        throw CliError(code: exitOther, message: "could not encode window list")
    }
    stdoutLine(text)
    return exitOK
}

func cmdActivate(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    guard let app = NSRunningApplication(processIdentifier: pid) else {
        throw CliError(code: exitOther, message: "no running application with pid \(pid)")
    }
    if #available(macOS 14.0, *) {
        app.activate()
    } else {
        app.activate(options: [.activateIgnoringOtherApps])
    }
    let deadline = Date().addingTimeInterval(2.0)
    while Date() < deadline {
        if NSWorkspace.shared.frontmostApplication?.processIdentifier == pid { return exitOK }
        Thread.sleep(forTimeInterval: 0.05)
    }
    throw CliError(code: exitOther, message: "pid \(pid) did not become frontmost within 2s")
}

func cmdKey(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    guard !args.positionals.isEmpty else {
        throw CliError(code: exitUsage, message: "key needs at least one combo")
    }
    // Resolve every combo first so a typo can't send half a sequence.
    let strokes = try args.positionals.map { combo -> (CGKeyCode, CGEventFlags, String) in
        let (code, flags) = try parseCombo(combo)
        return (code, flags, combo)
    }
    let sender = Sender(pid: pid)
    for (index, stroke) in strokes.enumerated() {
        if index > 0 { Thread.sleep(forTimeInterval: 0.040) }
        try sender.key(stroke.0, flags: stroke.1, stroke.2)
    }
    return exitOK
}

func cmdType(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    let text = args.positionals.joined(separator: " ")
    guard !text.isEmpty else {
        throw CliError(code: exitUsage, message: "type needs some text")
    }
    let sender = Sender(pid: pid)
    for (index, character) in text.enumerated() {
        if index > 0 { Thread.sleep(forTimeInterval: 0.015) }
        try sender.type(character)
    }
    return exitOK
}

func cmdClick(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    guard args.positionals.count == 2 else {
        throw CliError(code: exitUsage, message: "click needs <x> <y>")
    }
    let where_ = try origin(for: pid, args: args)
    let point = CGPoint(x: where_.x + (try number(args.positionals[0], "x")),
                        y: where_.y + (try number(args.positionals[1], "y")))
    let right = args.switches.contains("--right")
    let down: CGEventType = right ? .rightMouseDown : .leftMouseDown
    let up: CGEventType = right ? .rightMouseUp : .leftMouseUp
    let button: CGMouseButton = right ? .right : .left
    let sender = Sender(pid: pid)
    let clicks = args.switches.contains("--double") ? 2 : 1
    for click in 0..<clicks {
        if click > 0 { Thread.sleep(forTimeInterval: 0.080) }
        try sender.mouse(down, point, button, "click \(click + 1) down")
        try sender.mouse(up, point, button, "click \(click + 1) up")
    }
    return exitOK
}

func cmdDrag(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    guard args.positionals.count == 4 else {
        throw CliError(code: exitUsage, message: "drag needs <x1> <y1> <x2> <y2>")
    }
    var steps = 10
    if let raw = args.values["--steps"] {
        guard let parsed = Int(raw), parsed > 0 else {
            throw CliError(code: exitUsage, message: "--steps must be a positive integer (got '\(raw)')")
        }
        steps = parsed
    }
    let where_ = try origin(for: pid, args: args)
    let from = CGPoint(x: where_.x + (try number(args.positionals[0], "x1")),
                       y: where_.y + (try number(args.positionals[1], "y1")))
    let to = CGPoint(x: where_.x + (try number(args.positionals[2], "x2")),
                     y: where_.y + (try number(args.positionals[3], "y2")))
    let sender = Sender(pid: pid)
    try sender.mouse(.leftMouseDown, from, .left, "drag down")
    for step in 1...steps {
        let t = Double(step) / Double(steps)
        let point = CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t)
        try sender.mouse(.leftMouseDragged, point, .left, "drag move \(step)/\(steps)")
    }
    try sender.mouse(.leftMouseUp, to, .left, "drag up")
    return exitOK
}

func cmdScroll(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    guard args.positionals.count == 1 else {
        throw CliError(code: exitUsage, message: "scroll needs <dy>")
    }
    let delta = try number(args.positionals[0], "dy")
    guard let event = CGEvent(scrollWheelEvent2Source: CGEventSource(stateID: .hidSystemState),
                              units: .pixel, wheelCount: 1,
                              wheel1: Int32(delta.rounded()), wheel2: 0, wheel3: 0) else {
        throw CliError(code: exitOther, message: "could not create scroll event")
    }
    // The gate is checked here as well; scrolling needs a pointer position, so
    // honour --win by moving the cursor to the window first.
    let sender = Sender(pid: pid)
    if let id = try optionalWindowId(args) {
        let win = try window(of: pid, id: id)
        let point = CGPoint(x: win.rect.midX, y: win.rect.midY)
        try sender.mouse(.mouseMoved, point, .left, "scroll move")
    }
    try sender.post(event, "scroll")
    return exitOK
}

func cmdShot(_ args: Args) throws -> Int32 {
    let pid = try requiredPid(args)
    guard let out = args.values["--out"], !out.isEmpty else {
        throw CliError(code: exitUsage, message: "shot needs --out <file.png>")
    }
    let win: WinInfo
    if let id = try optionalWindowId(args) {
        win = try window(of: pid, id: id)
    } else {
        win = try frontWindow(of: pid, in: windowList(of: pid))
    }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    process.arguments = ["-x", "-o", "-l", String(win.id), out]
    do {
        try process.run()
    } catch {
        throw CliError(code: exitOther, message: "could not run screencapture: \(error.localizedDescription)")
    }
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw CliError(code: exitOther, message: "screencapture failed (status \(process.terminationStatus))")
    }
    stdoutLine(out)
    return exitOK
}

// MARK: - dispatch

func valueFlags(for command: String) -> Set<String> {
    switch command {
    case "windows", "activate", "key", "type": return ["--pid"]
    case "click": return ["--pid", "--win"]
    case "drag": return ["--pid", "--win", "--steps"]
    case "scroll": return ["--pid", "--win"]
    case "shot": return ["--pid", "--out", "--win"]
    default: return []
    }
}

func switchFlags(for command: String) -> Set<String> {
    switch command {
    case "click": return ["--right", "--double"]
    default: return []
    }
}

let commands: Set<String> = ["front", "windows", "activate", "key", "type",
                             "click", "drag", "scroll", "shot", "help", "-h", "--help"]

func run(_ argv: [String]) -> Int32 {
    guard let command = argv.first else {
        stderrLine(usage)
        return exitUsage
    }
    guard commands.contains(command) else {
        stderrLine("ks-input: unknown command '\(command)'")
        stderrLine(usage)
        return exitUsage
    }
    do {
        if command == "help" || command == "-h" || command == "--help" {
            stdoutLine(usage)
            return exitOK
        }
        let args = try parseArgs(Array(argv.dropFirst()),
                                 valueFlags: valueFlags(for: command),
                                 switchFlags: switchFlags(for: command))
        switch command {
        case "front": return cmdFront()
        case "windows": return try cmdWindows(args)
        case "activate": return try cmdActivate(args)
        case "key": return try cmdKey(args)
        case "type": return try cmdType(args)
        case "click": return try cmdClick(args)
        case "drag": return try cmdDrag(args)
        case "scroll": return try cmdScroll(args)
        case "shot": return try cmdShot(args)
        default: return exitUsage
        }
    } catch let error as CliError {
        stderrLine("ks-input: \(error.message)")
        if error.code == exitUsage { stderrLine(usage) }
        return error.code
    } catch {
        stderrLine("ks-input: \(error)")
        return exitOther
    }
}

exit(run(Array(CommandLine.arguments.dropFirst())))
