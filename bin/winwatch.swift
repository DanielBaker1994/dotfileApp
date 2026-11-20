import Cocoa
import CoreGraphics

// usage: swift winwatch.swift [delay=5] [duration=3] [interval=0.5]
// Logs the frontmost app + every on-screen window (pid, owner, title, size, layer).
let a = CommandLine.arguments

// --once: one sample, one line (for the kitchen-sink palette toast):
//   "focused: Ghostty pid 653 | tiny: Foo pid 123 28x40"
// Full dump goes to /tmp/winwatch.log. "Tiny" = layer 0, under 200pt either way,
// not the `borders` overlay.
if a.count > 1 && (a[1] == "--once" || a[1] == "--pids") {
    let front = NSWorkspace.shared.frontmostApplication
    var out = "focused: \(front?.localizedName ?? "?") pid \(front?.processIdentifier ?? -1)"
    var dump = ""
    var tiny: [String] = []
    var tinyRows: [String] = []
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list {
        let pid = w[kCGWindowOwnerPID as String] as? Int ?? -1
        let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
        let title = w[kCGWindowName as String] as? String ?? ""
        let layer = w[kCGWindowLayer as String] as? Int ?? 0
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        let ww = Int(b["Width"] as? Double ?? 0), hh = Int(b["Height"] as? Double ?? 0)
        dump += "pid=\(pid) owner=\(owner) layer=\(layer) \(ww)x\(hh) title=\"\(title)\"\n"
        if layer == 0 && (ww < 200 || hh < 200) && owner != "borders" { tiny.append("\(owner) pid \(pid) \(ww)x\(hh)"); tinyRows.append("T\t\(pid)\t\(owner)\t\(ww)x\(hh)") }
    }
    try? dump.write(toFile: "/tmp/winwatch.log", atomically: true, encoding: .utf8)
    if !tiny.isEmpty { out += " | tiny: " + tiny.joined(separator: ", ") }
    if a[1] == "--pids" {
        // machine-readable (kitchen-sink's kill dialog): F<TAB>pid<TAB>name, then T<TAB>pid<TAB>owner<TAB>WxH per tiny window
        print("F\t\(front?.processIdentifier ?? -1)\t\(front?.localizedName ?? "?")")
        for r in tinyRows { print(r) }
        exit(0)
    }
    print(out)
    exit(0)
}

// --pids2: candidate windows for kitchen-sink's kill picker, one per line:
//   <kind>\t<pid>\t<owner>\t<WxH>\t<note>
// kinds: P = under the mouse pointer (front to back, up to 3), A = AX focused
// window, F = NSWorkspace frontmost app, T = small window (any layer, any app).
// "Point at it" (P) is the reliable one for orphaned windows: their process is
// often not an activatable app, so it never becomes frontmost or AX-focused.
// KS_EXCLUDE_PID (kitchen-sink's own pid) is skipped everywhere.
if a.count > 1 && a[1] == "--pids2" {
    let exclude = Int32(ProcessInfo.processInfo.environment["KS_EXCLUDE_PID"] ?? "") ?? -1
    let me = getpid()
    let skipOwners: Set<String> = ["Window Server", "Dock", "borders", "WindowManager", "Control Center", "Control Centre", "Notification Center", "Notification Centre"]
    func skip(_ pid: Int32) -> Bool { pid == exclude || pid == me || pid <= 1 }
    // What macOS thinks the process is: a regular app, a background agent, or no app at all.
    func note(_ pid: Int32, _ extra: [String] = []) -> String {
        var n = extra
        if let app = NSRunningApplication(processIdentifier: pid) {
            if app.activationPolicy == .accessory { n.append("agent") }
            if app.activationPolicy == .prohibited { n.append("background") }
            if app.isHidden { n.append("app hidden") }
        } else {
            n.append("not an app")
        }
        return n.joined(separator: ", ")
    }
    func row(_ k: String, _ pid: Int32, _ owner: String, _ size: String, _ extra: [String] = []) {
        print("\(k)\t\(pid)\t\(owner.replacingOccurrences(of: "\t", with: " "))\t\(size)\t\(note(pid, extra))")
    }

    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    var dump = ""
    // Mouse in CG global coords (top-left origin), same space as kCGWindowBounds.
    let mouse = CGEvent(source: nil)?.location ?? .zero
    var under = 0
    var tinySeen = Set<Int32>()
    var tinyRows: [String] = []
    for w in list {
        let pid = Int32(w[kCGWindowOwnerPID as String] as? Int ?? -1)
        let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
        let layer = w[kCGWindowLayer as String] as? Int ?? 0
        let alpha = w[kCGWindowAlpha as String] as? Double ?? 1
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        let r = CGRect(x: b["X"] as? Double ?? 0, y: b["Y"] as? Double ?? 0,
                       width: b["Width"] as? Double ?? 0, height: b["Height"] as? Double ?? 0)
        dump += "pid=\(pid) owner=\(owner) layer=\(layer) alpha=\(alpha) \(Int(r.width))x\(Int(r.height))@\(Int(r.minX)),\(Int(r.minY))\n"
        if skip(pid) || skipOwners.contains(owner) || layer < 0 || r.width < 1 || r.height < 1 { continue }
        let size = "\(Int(r.width))x\(Int(r.height))"
        var extra: [String] = []
        if alpha < 0.05 { extra.append("invisible") }
        if layer != 0 { extra.append("layer \(layer)") }
        if under < 3 && r.contains(mouse) {
            row("P", pid, owner, size, extra)
            under += 1
        }
        // Menu bar status items (layer 25 along the top) would flood the list.
        let statusItem = layer == 25 && r.minY < 40
        if (r.width < 200 || r.height < 200) && !statusItem && !tinySeen.contains(pid) {
            tinySeen.insert(pid)
            tinyRows.append("\(pid)\t\(owner)\t\(size)\t\(extra.joined(separator: ","))")
        }
    }
    try? dump.write(toFile: "/tmp/winwatch.log", atomically: true, encoding: .utf8)

    // AX focus: asks the window server's focus, so it also sees non-activating
    // panels. Needs Accessibility for the responsible app (kitchen-sink); if it
    // isn't granted this just yields nothing.
    let sys = AXUIElementCreateSystemWide()
    var appRef: CFTypeRef?
    if AXUIElementCopyAttributeValue(sys, kAXFocusedApplicationAttribute as CFString, &appRef) == .success, let app = appRef {
        let appEl = app as! AXUIElement
        var winRef: CFTypeRef?
        var pid: pid_t = -1
        if AXUIElementCopyAttributeValue(appEl, kAXFocusedWindowAttribute as CFString, &winRef) == .success, let win = winRef {
            AXUIElementGetPid(win as! AXUIElement, &pid)
        } else {
            AXUIElementGetPid(appEl, &pid)
        }
        if !skip(pid) {
            let name = NSRunningApplication(processIdentifier: pid)?.localizedName ?? "pid \(pid)"
            row("A", pid, name, "-")
        }
    }
    if let front = NSWorkspace.shared.frontmostApplication, !skip(front.processIdentifier) {
        row("F", front.processIdentifier, front.localizedName ?? "?", "-")
    }
    for t in tinyRows {
        let f = t.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
        let pid = Int32(f[0]) ?? -1
        row("T", pid, f[1], f[2], f[3].isEmpty ? [] : f[3].components(separatedBy: ","))
    }
    exit(0)
}
let delay = a.count > 1 ? Double(a[1]) ?? 5 : 5
let dur = a.count > 2 ? Double(a[2]) ?? 3 : 3
let step = a.count > 3 ? Double(a[3]) ?? 0.5 : 0.5

Thread.sleep(forTimeInterval: delay)
let end = Date().addingTimeInterval(dur)
let df = DateFormatter(); df.dateFormat = "HH:mm:ss.SSS"

while Date() < end {
    let front = NSWorkspace.shared.frontmostApplication
    print("[\(df.string(from: Date()))] FRONT pid=\(front?.processIdentifier ?? -1) name=\(front?.localizedName ?? "?")")
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list {
        let pid = w[kCGWindowOwnerPID as String] as? Int ?? -1
        let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
        let title = w[kCGWindowName as String] as? String ?? ""
        let layer = w[kCGWindowLayer as String] as? Int ?? 0
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        let ww = b["Width"] as? Double ?? 0, hh = b["Height"] as? Double ?? 0
        let x = b["X"] as? Double ?? 0, y = b["Y"] as? Double ?? 0
        print("  pid=\(pid) owner=\(owner) layer=\(layer) \(Int(ww))x\(Int(hh))@\(Int(x)),\(Int(y)) title=\"\(title)\"")
    }
    fflush(stdout)
    Thread.sleep(forTimeInterval: step)
}
