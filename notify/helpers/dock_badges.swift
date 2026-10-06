// dock_badges: print the badge the Dock shows on each app icon, one line per
// Dock item: "<bundle id>\t<badge>" (badge empty = none). Read through the
// Dock's accessibility tree (AXStatusLabel), i.e. exactly what you see — this
// covers badges `lsappinfo` misses (UserNotifications badges, e.g. Messages).
// Needs Accessibility for the process that runs it (the workspace-switcher app).
// Built on demand by notify/notify_poll.py into ~/.cache/workspace-switcher/helpers/.
import AppKit
import ApplicationServices

guard let dock = NSRunningApplication.runningApplications(
        withBundleIdentifier: "com.apple.dock").first else { exit(1) }
let app = AXUIElementCreateApplication(dock.processIdentifier)

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}

guard AXIsProcessTrusted() else {
    FileHandle.standardError.write("dock_badges: no Accessibility permission\n".data(using: .utf8)!)
    exit(2)
}
var out = ""
for list in (attr(app, kAXChildrenAttribute) as? [AXUIElement]) ?? [] {
    for item in (attr(list, kAXChildrenAttribute) as? [AXUIElement]) ?? [] {
        guard let url = attr(item, "AXURL") as? URL,
              let id = Bundle(url: url)?.bundleIdentifier else { continue }
        let label = (attr(item, "AXStatusLabel") as? String) ?? ""
        out += "\(id)\t\(label)\n"
    }
}
print(out, terminator: "")
