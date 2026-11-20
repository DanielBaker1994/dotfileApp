import Cocoa
import CoreGraphics

// usage: swift winwatch.swift [delay=5] [duration=3] [interval=0.5]
// Logs the frontmost app + every on-screen window (pid, owner, title, size, layer).
let a = CommandLine.arguments

// --once: one sample, one line (for the kitchen-sink palette toast):
//   "focused: Ghostty pid 653 | tiny: Foo pid 123 28x40"
// Full dump goes to /tmp/winwatch.log. "Tiny" = layer 0, under 200pt either way,
// not the `borders` overlay.
if a.count > 1 && a[1] == "--once" {
    let front = NSWorkspace.shared.frontmostApplication
    var out = "focused: \(front?.localizedName ?? "?") pid \(front?.processIdentifier ?? -1)"
    var dump = ""
    var tiny: [String] = []
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list {
        let pid = w[kCGWindowOwnerPID as String] as? Int ?? -1
        let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
        let title = w[kCGWindowName as String] as? String ?? ""
        let layer = w[kCGWindowLayer as String] as? Int ?? 0
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        let ww = Int(b["Width"] as? Double ?? 0), hh = Int(b["Height"] as? Double ?? 0)
        dump += "pid=\(pid) owner=\(owner) layer=\(layer) \(ww)x\(hh) title=\"\(title)\"\n"
        if layer == 0 && (ww < 200 || hh < 200) && owner != "borders" { tiny.append("\(owner) pid \(pid) \(ww)x\(hh)") }
    }
    try? dump.write(toFile: "/tmp/winwatch.log", atomically: true, encoding: .utf8)
    if !tiny.isEmpty { out += " | tiny: " + tiny.joined(separator: ", ") }
    print(out)
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
